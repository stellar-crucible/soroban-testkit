//! Budget and storage: what a staking call costs and where the pool puts it.
//!
//! The ceilings are transactions' headroom, not a benchmark: a contract
//! registered natively is metered without the VM's own costs, so these numbers
//! are a property of this contract's accounting rather than of a network fee.

mod common;

use common::{Fixture, ONE};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetMetric, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};

#[test]
fn a_stake_stays_inside_its_transaction_ceiling() {
    let fixture = Fixture::new();

    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "stake",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || { fixture.client().stake(&fixture.alice, &(100 * ONE)); }
    );
}

#[test]
fn a_claim_stays_inside_its_transaction_ceiling() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);

    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "claim",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || { fixture.client().claim(&fixture.alice); }
    );
}

#[test]
fn a_view_measures_less_than_a_write() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);

    fixture.client().earned(&fixture.alice);
    let view = BudgetSnapshot::last_invocation(&fixture.env);
    fixture.client().claim(&fixture.alice);
    let write = BudgetSnapshot::last_invocation(&fixture.env);

    assert!(view.cpu_insns < write.cpu_insns);
}

#[test]
fn the_pool_lives_in_instance_storage() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);

    soroban_testkit_core::assert_storage_tier!(
        &fixture.env,
        &fixture.id,
        "pool",
        StorageTier::Instance
    );
}

#[test]
fn a_position_lives_in_persistent_storage() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);

    let entries = StorageSnapshot::capture(&fixture.env, &fixture.id);
    let position = entries
        .entries()
        .iter()
        .find(|entry| entry.key.contains("position"))
        .expect("a stake writes a position entry");

    assert_eq!(
        position.tier,
        StorageTier::Persistent,
        "a position outlives the instance entry"
    );
}

#[test]
fn a_stake_adds_one_persistent_entry() {
    let fixture = Fixture::new();
    let before = StorageSnapshot::capture(&fixture.env, &fixture.id);

    fixture.stake(&fixture.alice, 10 * ONE);
    let after = StorageSnapshot::capture(&fixture.env, &fixture.id);

    let diff = before.diff(&after);
    assert_eq!(diff.added().len(), 1, "{diff:?}");
    assert_eq!(
        diff.removed().len(),
        0,
        "staking removes nothing from storage"
    );
}

#[test]
fn a_top_up_rewrites_the_same_position_entry() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    let before = StorageSnapshot::capture(&fixture.env, &fixture.id);

    fixture.stake(&fixture.alice, 5 * ONE);
    let after = StorageSnapshot::capture(&fixture.env, &fixture.id);

    let diff = before.diff(&after);
    assert_eq!(diff.added().len(), 0, "{diff:?}");
    // The instance entry holding the pool and the position entry both change;
    // no second position is created.
    assert_eq!(diff.modified().len(), 2, "{diff:?}");
}

#[test]
fn a_view_does_not_write_storage() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    fixture.advance_time(50);
    let before = StorageSnapshot::capture(&fixture.env, &fixture.id);

    fixture.client().earned(&fixture.alice);
    fixture.client().staked(&fixture.alice);
    fixture.client().total_staked();

    before.assert_unchanged(&StorageSnapshot::capture(&fixture.env, &fixture.id));
}

#[test]
fn a_full_exit_zeroes_the_position_without_removing_it() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    fixture.advance_time(7 * 24 * 60 * 60);

    fixture.client().unstake(&fixture.alice, &(10 * ONE));

    let after = StorageSnapshot::capture(&fixture.env, &fixture.id);
    assert!(
        after
            .entries()
            .iter()
            .any(|entry| entry.key.contains("position")),
        "the position entry is kept, zeroed"
    );
    assert_eq!(fixture.client().staked(&fixture.alice), 0);
    assert_eq!(fixture.client().total_staked(), 0);
}

#[test]
fn a_baseline_of_the_hot_paths_round_trips_through_its_file_format() {
    let fixture = Fixture::new();
    let mut baseline = BudgetBaseline::new();

    fixture.client().stake(&fixture.alice, &(10 * ONE));
    baseline.record("stake", BudgetSnapshot::last_invocation(&fixture.env));
    fixture.client().unstake(&fixture.alice, &(10 * ONE));
    baseline.record("unstake", BudgetSnapshot::last_invocation(&fixture.env));

    let reloaded = BudgetBaseline::parse(&baseline.to_json_string()).unwrap();

    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded.get("stake"), baseline.get("stake"));
    for (case, cost) in reloaded.cases() {
        assert!(cost.cpu_insns > 0, "{case} was measured");
        assert!(cost.mem_bytes > 0, "{case} was measured");
    }
}

#[test]
fn the_guard_rejects_a_cost_past_the_recorded_baseline() {
    let fixture = Fixture::new();
    fixture.client().stake(&fixture.alice, &(10 * ONE));

    let guard = BudgetBaseline::new()
        .guard("stake")
        .baseline(Some(BudgetSnapshot {
            cpu_insns: 1,
            mem_bytes: 1,
        }))
        .tolerance(0);
    let violations = guard.violations(&BudgetSnapshot::last_invocation(&fixture.env));

    assert!(violations
        .iter()
        .any(|violation| violation.metric == BudgetMetric::Cpu));
}
