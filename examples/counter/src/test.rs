extern crate std;

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

    // Warm up the cost estimator so a budget snapshot is available.
    client.get();

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
    client.get();

    soroban_testkit_core::budget_guard!(&ctx.env, "increment", { cpu_max: 1 }, || {
        client.increment(&caller, &1)
    });
}

#[test]
fn a_recorded_baseline_accepts_the_same_cost_and_flags_a_cheaper_one() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();
    client.get();

    let env = ctx.env.clone();
    let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
    client.increment(&caller, &1);
    let measured = before.diff(&BudgetSnapshot::capture(&env.cost_estimate().budget()));

    let mut baseline = BudgetBaseline::new();
    baseline.record("increment", measured);

    // The same call measured the same way stays inside its own tolerance.
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
