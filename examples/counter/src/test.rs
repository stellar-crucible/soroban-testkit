extern crate std;

// The crate is `no_std`, so the baseline helpers name the std items they use.
use std::{println, string::ToString, vec::Vec};

use soroban_sdk::Address;
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_core::budget::{BudgetBaseline, BudgetMetric, BudgetSnapshot};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

use crate::{CounterContract, CounterContractClient};

fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

fn client_for(ctx: &TestContext) -> (Address, CounterContractClient<'_>) {
    let contract_id = ctx.env.register(CounterContract, ());
    (
        contract_id.clone(),
        CounterContractClient::new(&ctx.env, &contract_id),
    )
}

#[test]
fn counter_starts_at_zero() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);

    assert_eq!(client.get(), 0);
}

#[test]
fn increment_accumulates_and_returns_the_new_value() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller: Address = ctx.users[0].clone();

    assert_eq!(client.increment(&caller, &5), 5);
    assert_eq!(client.increment(&caller, &7), 12);
    assert_eq!(client.get(), 12);
}

#[test]
fn increment_emits_exactly_one_event() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);

    EventMatcher::new(&ctx.env).assert_count(1);
    EventMatcher::new(&ctx.env).assert_emitted();
}

#[test]
fn read_only_calls_emit_nothing() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);

    client.get();

    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn a_topic_the_contract_never_publishes_stays_silent() {
    let ctx = context(1);
    let (contract_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);

    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("decremented")
        .assert_not_emitted();
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("incremented")
        .assert_emitted();
}

#[test]
fn each_invocation_is_asserted_on_its_own() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);
    EventMatcher::new(&ctx.env).assert_count(1);

    client.increment(&caller, &2);
    EventMatcher::new(&ctx.env).assert_count(1);
}

#[test]
fn events_can_be_filtered_to_the_contract_that_emitted_them() {
    let ctx = context(1);
    let (contract_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);

    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("counter")
        .assert_emitted();
}

#[test]
fn increment_stays_inside_its_cpu_ceiling() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    // A ceiling is transaction headroom, not the measured cost: a native test
    // contract is metered without the VM's own costs, so the call reads tens of
    // thousands of instructions here and a network would bill more.
    soroban_testkit_core::budget_guard!(
        &ctx.env,
        "increment",
        { cpu_max: 20_000_000, mem_max: 20_000_000 },
        || client.increment(&caller, &1)
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=increment metric=cpu_insns")]
fn a_call_past_its_ceiling_fails_the_test_with_a_parseable_line() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    soroban_testkit_core::budget_guard!(&ctx.env, "increment", { cpu_max: 1 }, || {
        client.increment(&caller, &1)
    });
}

#[test]
fn a_recorded_baseline_accepts_the_same_cost_and_flags_a_cheaper_one() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);
    let measured = BudgetSnapshot::last_invocation(&ctx.env);

    let mut baseline = BudgetBaseline::new();
    baseline.record("increment", measured);

    // The guard judges the invocation it wrapped, so the same call again is
    // inside its own recorded cost.
    baseline
        .guard("increment")
        .tolerance_percent(25)
        .run(&ctx.env, || {
            client.increment(&caller, &1);
        });

    // A baseline recorded from a build that was half as expensive is a regression.
    let halved = BudgetSnapshot {
        cpu_insns: measured.cpu_insns / 2,
        mem_bytes: measured.mem_bytes / 2,
    };
    let violations = baseline
        .guard("increment")
        .baseline(Some(halved))
        .tolerance_percent(10)
        .violations(&measured);
    assert!(
        violations
            .iter()
            .any(|violation| violation.metric == BudgetMetric::Cpu),
        "a call costing twice its baseline must be reported as growth"
    );
}

#[test]
#[should_panic]
fn increment_requires_authorization_when_auths_are_not_mocked() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);
}

// The two tests below are `#[ignore]`d on purpose. Soroban metering is
// reproducible — this baseline, recorded on Windows, measured identical on the
// `ubuntu-latest` runner — but the numbers still move with the SDK and env-host
// versions, so a local run against a different lockfile would fail for a change
// that cost nothing. One test writes into the repository as well. The
// `Budget baseline` workflow runs both where `Cargo.lock` and the runner are
// pinned, and compares against the committed file.

fn baseline_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

/// Run one call against a contract that already holds its entry, and return the
/// metering that call left behind. The prime makes both cases steady-state — a
/// read of a live counter, a rewrite of one — so recording and checking measure
/// the same shape of call.
fn measure(invocation: impl Fn(&CounterContractClient<'_>, &Address)) -> BudgetSnapshot {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);
    invocation(&client, &caller);

    BudgetSnapshot::last_invocation(&ctx.env)
}

fn hot_paths() -> [(&'static str, BudgetSnapshot); 2] {
    [
        (
            "get",
            measure(|client, _| {
                client.get();
            }),
        ),
        (
            "increment",
            measure(|client, caller| {
                client.increment(caller, &1);
            }),
        ),
    ]
}

/// One parseable line per metric, so the job summary is a cost table rather
/// than a paragraph.
fn report(case: &str, cost: &BudgetSnapshot, recorded: &BudgetSnapshot) {
    let metrics = [
        (BudgetMetric::Cpu, cost.cpu_insns, recorded.cpu_insns),
        (BudgetMetric::Memory, cost.mem_bytes, recorded.mem_bytes),
    ];
    for (metric, actual, start) in metrics {
        let drift = if start == 0 {
            0.0
        } else {
            (actual as f64 - start as f64) * 100.0 / start as f64
        };
        println!(
            "BUDGET_SUMMARY case={case} metric={} actual={actual} baseline={start} drift={drift:+.2}%",
            metric.as_str()
        );
    }
}

#[test]
#[ignore = "rewrites examples/counter/budget.json — run it deliberately, then commit the file"]
fn budget_baseline_records_current_costs() {
    let mut baseline = BudgetBaseline::new();
    for (case, cost) in hot_paths() {
        baseline.record(case, cost);
    }

    baseline.save(&baseline_path()).unwrap();
    // Printed so a recording run in CI shows the file it just wrote.
    println!("{}", baseline.to_json_string());
}

#[test]
#[ignore = "enforced by the Budget baseline workflow on a pinned runner"]
fn budget_baseline_rejects_drifted_costs() {
    let recorded = BudgetBaseline::load(&baseline_path()).expect(
        "examples/counter/budget.json is missing; run budget_baseline_records_current_costs",
    );

    let tolerance = std::env::var("TESTKIT_BUDGET_TOLERANCE")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);

    let mut breaches = Vec::new();
    for (case, cost) in hot_paths() {
        match recorded.get(case) {
            Some(start) => {
                report(case, &cost, &start);
                breaches.extend(
                    recorded
                        .guard(case)
                        .tolerance_percent(tolerance)
                        .violations(&cost),
                );
            }
            None => println!("BUDGET_SUMMARY case={case} status=unrecorded"),
        }
    }

    if !breaches.is_empty() {
        let lines = breaches
            .iter()
            .map(|breach| breach.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        panic!("budget drifted past {tolerance}%:\n{lines}");
    }
}
