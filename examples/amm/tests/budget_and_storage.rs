//! soroban-testkit-core: budget baselines for the swap and liquidity hot
//! paths, storage snapshot diffs over reserve updates, entry TTLs, and the
//! cross-contract call tracer on a routed swap.
//!
//! The baseline lives in `examples/amm/budget.json`. Refresh it with
//! `cargo test -p soroban-testkit-example-amm --test budget_and_storage -- --ignored record_budget_baseline`
//! and commit the file.

mod common;

use std::path::{Path, PathBuf};

use common::{Amm, ONE};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_core::tracer::{CallOutcome, CallTracer};
use soroban_testkit_core::ttl::{TtlExtension, TtlInspector};
use soroban_testkit_core::{assert_storage_tier, budget_guard, StorageTier};
use soroban_testkit_example_amm::{PoolKey, TokenKey, DEFAULT_FEE_BPS};

/// Growth over the committed baseline a measurement may show before failing.
const TOLERANCE_PERCENT: u64 = 20;

struct Fixture {
    env: Env,
    amm: Amm,
    lp: Address,
    trader: Address,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let amm = Amm::deploy(&env, &admin, DEFAULT_FEE_BPS);
    let lp = Address::generate(&env);
    let trader = Address::generate(&env);
    amm.fund(&lp, 1_000_000 * ONE);
    amm.fund(&trader, 1_000_000 * ONE);
    amm.pool()
        .deposit(&lp, &(100_000 * ONE), &(100_000 * ONE), &0, &0);
    Fixture {
        env,
        amm,
        lp,
        trader,
    }
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

/// Run every hot path once and measure it.
fn measure_hot_paths() -> Vec<(&'static str, BudgetSnapshot)> {
    let fx = setup();
    let mut measured = Vec::new();

    fx.amm.pool().swap(&fx.trader, &true, &(1_000 * ONE), &0);
    measured.push(("swap", BudgetSnapshot::last_invocation(&fx.env)));

    fx.amm
        .router()
        .swap_exact_in(&fx.amm.pool, &fx.trader, &false, &(1_000 * ONE), &0);
    measured.push(("router_swap", BudgetSnapshot::last_invocation(&fx.env)));

    fx.amm
        .pool()
        .deposit(&fx.lp, &(5_000 * ONE), &(5_000 * ONE), &0, &0);
    measured.push(("deposit", BudgetSnapshot::last_invocation(&fx.env)));

    let shares = fx.amm.share().balance(&fx.lp);
    fx.amm.pool().withdraw(&fx.lp, &(shares / 2), &0, &0);
    measured.push(("withdraw", BudgetSnapshot::last_invocation(&fx.env)));

    measured
}

// --- budget -----------------------------------------------------------------

#[test]
fn every_hot_path_stays_within_the_committed_baseline() {
    let baseline = BudgetBaseline::load(&baseline_path())
        .expect("examples/amm/budget.json must exist and parse");

    for (case, cost) in measure_hot_paths() {
        assert!(
            baseline.get(case).is_some(),
            "budget.json has no `{case}` case; run record_budget_baseline"
        );
        baseline
            .guard(case)
            .tolerance(TOLERANCE_PERCENT)
            .assert_within(&cost);
    }
}

#[test]
#[ignore = "rewrites examples/amm/budget.json — run deliberately, then commit the file"]
fn record_budget_baseline() {
    let mut baseline = BudgetBaseline::new();
    for (case, cost) in measure_hot_paths() {
        baseline.record(case, cost);
    }
    baseline.save(&baseline_path()).unwrap();
}

#[test]
fn a_routed_swap_costs_more_than_a_direct_one_but_not_twice_as_much() {
    let costs = measure_hot_paths();
    let cost = |name| costs.iter().find(|(case, _)| *case == name).unwrap().1;

    let direct = cost("swap");
    let routed = cost("router_swap");
    assert!(
        routed.cpu_insns > direct.cpu_insns,
        "the router adds a frame"
    );
    assert!(
        routed.cpu_insns < direct.cpu_insns * 2,
        "a router hop must stay cheaper than a second swap: {} vs {}",
        routed.cpu_insns,
        direct.cpu_insns
    );
}

#[test]
fn a_swap_fits_under_a_hard_cpu_and_memory_ceiling() {
    let fx = setup();

    let out = budget_guard!(&fx.env, "swap", { cpu_max: 2_000_000, mem_max: 1_000_000 }, || {
        fx.amm.pool().swap(&fx.trader, &true, &(1_000 * ONE), &0)
    });

    assert!(out > 0);
}

// --- storage ----------------------------------------------------------------

#[test]
fn a_swap_changes_only_the_two_reserves_in_pool_storage() {
    let fx = setup();
    let before = StorageSnapshot::capture(&fx.env, &fx.amm.pool);

    fx.amm.pool().swap(&fx.trader, &true, &(1_000 * ONE), &0);
    let diff = before.diff(&StorageSnapshot::capture(&fx.env, &fx.amm.pool));

    assert!(diff.added().is_empty() && diff.removed().is_empty());
    let mut changed: Vec<&str> = diff.modified().iter().map(|c| c.key.as_str()).collect();
    changed.sort();
    assert_eq!(changed, ["[ReserveA]", "[ReserveB]"]);
}

#[test]
fn a_first_time_depositor_adds_one_share_balance_entry() {
    let fx = setup();
    let newcomer = Address::generate(&fx.env);
    fx.amm.fund(&newcomer, 10_000 * ONE);
    let before = StorageSnapshot::capture(&fx.env, &fx.amm.share);

    fx.amm
        .pool()
        .deposit(&newcomer, &(1_000 * ONE), &(1_000 * ONE), &0, &0);
    let diff = before.diff(&StorageSnapshot::capture(&fx.env, &fx.amm.share));

    assert_eq!(diff.added().len(), 1, "one new LP balance");
    assert_eq!(diff.added()[0].tier, StorageTier::Persistent);
    assert!(
        diff.added()[0].key.starts_with("[Balance,"),
        "{}",
        diff.added()[0].key
    );
}

#[test]
fn pool_configuration_and_reserves_live_in_instance_storage() {
    let fx = setup();

    assert_storage_tier!(&fx.env, &fx.amm.pool, "[Reserve*", StorageTier::Instance);
    assert_storage_tier!(
        &fx.env,
        &fx.amm.pool,
        "[TotalShares]",
        StorageTier::Instance
    );
    assert_storage_tier!(
        &fx.env,
        &fx.amm.token_a,
        "[Balance*",
        StorageTier::Persistent
    );
}

#[test]
fn balances_carry_the_persistent_ttl_and_the_pool_shares_the_instance_ttl() {
    let fx = setup();

    let balance = TtlInspector::for_key(&fx.env, &fx.amm.share, &TokenKey::Balance(fx.lp.clone()));
    let reserves = TtlInspector::for_key(&fx.env, &fx.amm.pool, &PoolKey::ReserveA);

    assert_eq!(balance.tier, StorageTier::Persistent);
    assert_eq!(reserves.tier, StorageTier::Instance);
    assert_eq!(
        reserves.live_until_ledger,
        TtlInspector::instance(&fx.env, &fx.amm.pool).live_until_ledger
    );
    assert!(balance.ttl() >= balance.min_ttl() - 1);
}

#[test]
fn rewriting_a_balance_does_not_extend_its_ttl() {
    let fx = setup();

    let extension = TtlExtension::observe(
        &fx.env,
        &fx.amm.token_a,
        &TokenKey::Balance(fx.trader.clone()),
        || fx.amm.pool().swap(&fx.trader, &false, &(1_000 * ONE), &0),
    );

    extension.assert_not_extended();
}

// --- call tracing -----------------------------------------------------------

fn labelled(trace: CallTracer, amm: &Amm) -> CallTracer {
    trace
        .with_label(&amm.router, "router")
        .with_label(&amm.pool, "pool")
        .with_label(&amm.token_a, "token_a")
        .with_label(&amm.token_b, "token_b")
        .with_label(&amm.share, "share")
}

#[test]
fn a_routed_swap_is_three_frames_deep() {
    let fx = setup();

    let trace = labelled(
        CallTracer::capture(&fx.env, || {
            fx.amm
                .router()
                .swap_exact_in(&fx.amm.pool, &fx.trader, &true, &(1_000 * ONE), &0)
        }),
        &fx.amm,
    );

    trace.assert_depth(3);
    trace.assert_success();
    let names: Vec<String> = trace.frames().iter().map(|f| trace.call_name(f)).collect();
    assert_eq!(
        names,
        [
            "router::swap_exact_in",
            "pool::swap",
            "token_b::transfer",
            "token_a::transfer"
        ]
    );
    let root = trace.root().unwrap();
    assert!(root.cpu_insns > root.children[0].cpu_insns);
}

#[test]
fn a_deposit_fans_out_to_three_token_calls() {
    let fx = setup();

    let trace = labelled(
        CallTracer::capture(&fx.env, || {
            fx.amm
                .pool()
                .deposit(&fx.lp, &(1_000 * ONE), &(1_000 * ONE), &0, &0)
        }),
        &fx.amm,
    );

    trace.assert_depth(2);
    assert_eq!(trace.root().unwrap().children.len(), 3);
    assert_eq!(trace.calls_to(&fx.amm.share, "mint").len(), 1);
    let ascii = trace.render_ascii();
    assert!(ascii.contains("├── token_a::transfer [ok]"), "{ascii}");
    assert!(ascii.contains("└── share::mint [ok]"), "{ascii}");
}

#[test]
fn a_failed_routed_swap_points_at_the_token_that_refused() {
    let fx = setup();
    let broke = Address::generate(&fx.env);

    let trace = labelled(
        CallTracer::capture(&fx.env, || {
            fx.amm
                .router()
                .swap_exact_in(&fx.amm.pool, &broke, &true, &ONE, &0)
        }),
        &fx.amm,
    );

    assert!(!trace.succeeded());
    assert_eq!(
        trace.render_failure_path().unwrap(),
        "router::swap_exact_in -> pool::swap -> token_b::transfer (error: Contract(101))"
    );
    let failing = trace.failure_path().unwrap();
    assert_eq!(
        failing.last().unwrap().outcome,
        CallOutcome::Error(Some(soroban_sdk::xdr::ScError::Contract(101)))
    );
    let mermaid = trace.render_mermaid();
    assert!(mermaid.contains("participant C0 as router"), "{mermaid}");
    assert!(mermaid.contains("--xC1: error: Contract(101)"), "{mermaid}");
}
