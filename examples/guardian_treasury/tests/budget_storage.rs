mod common;

use std::path::{Path, PathBuf};

use common::{Fixture, TIMELOCK};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_core::StorageTier as CoreStorageTier;

fn snapshot(fixture: &Fixture) -> BudgetSnapshot {
    BudgetSnapshot::capture(&fixture.ctx.env.host().budget_cloned())
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

fn measure_hot_paths() -> Vec<(&'static str, BudgetSnapshot)> {
    let veto_fixture = Fixture::funded();
    let id = veto_fixture.ready_proposal(250);
    veto_fixture.client().veto(&veto_fixture.guardian, &id);
    let veto = BudgetSnapshot::last_invocation(&veto_fixture.ctx.env);

    let execute_fixture = Fixture::funded();
    let id = execute_fixture.ready_proposal(250);
    execute_fixture.advance_time(TIMELOCK);
    execute_fixture
        .client()
        .execute(&execute_fixture.signers[0], &id);
    let execute = BudgetSnapshot::last_invocation(&execute_fixture.ctx.env);

    vec![("guardian_veto", veto), ("execute", execute)]
}

#[test]
fn guardian_veto_and_execution_match_committed_budget_baseline() {
    let baseline = BudgetBaseline::load(&baseline_path())
        .expect("examples/guardian_treasury/budget.json must exist and parse");
    for (case, cost) in measure_hot_paths() {
        baseline
            .guard(case)
            .tolerance_percent(25)
            .assert_within(&cost);
    }
}

#[test]
#[ignore = "rewrites the committed guardian treasury budget baseline"]
fn record_budget_baseline() {
    let mut baseline = BudgetBaseline::new();
    for (case, cost) in measure_hot_paths() {
        baseline.record(case, cost);
    }
    baseline
        .save(&baseline_path())
        .expect("guardian treasury budget baseline should be writable");
}

#[test]
fn deposit_consumes_cpu_or_memory() {
    let fixture = Fixture::initialized();
    let before = snapshot(&fixture);
    fixture.fund(500);
    let diff = before.diff(&snapshot(&fixture));
    assert!(diff.cpu_insns > 0 || diff.mem_bytes > 0);
}

#[test]
fn proposal_approval_and_veto_are_metered() {
    let fixture = Fixture::funded();
    let id = fixture.propose(100);
    fixture.client().approve(&fixture.signers[0], &id);
    fixture.client().approve(&fixture.signers[1], &id);
    let before = snapshot(&fixture);
    fixture.client().veto(&fixture.guardian, &id);
    let diff = before.diff(&snapshot(&fixture));
    assert!(diff.cpu_insns > 0 || diff.mem_bytes > 0);
}

#[test]
fn execution_has_a_committed_cpu_budget_ceiling() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(250);
    fixture.advance_time(TIMELOCK);
    BudgetGuard::new("guardian_treasury.execute")
        .cpu_ceiling(100_000_000)
        .run(&fixture.ctx.env, || {
            fixture.client().execute(&fixture.signers[0], &id)
        });
    let cost = BudgetSnapshot::last_invocation(&fixture.ctx.env);
    assert!(cost.cpu_insns > 0);
}

#[test]
fn veto_has_a_committed_cpu_budget_ceiling() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(250);
    BudgetGuard::new("guardian_treasury.veto")
        .cpu_ceiling(100_000_000)
        .run(&fixture.ctx.env, || {
            fixture.client().veto(&fixture.guardian, &id)
        });
    let cost = BudgetSnapshot::last_invocation(&fixture.ctx.env);
    assert!(cost.cpu_insns > 0);
}

#[test]
fn initialization_and_proposals_use_expected_storage_tiers() {
    let fixture = Fixture::funded();
    let id = fixture.propose(100);
    let storage = StorageSnapshot::capture(&fixture.ctx.env, &fixture.contract_id);
    assert!(storage
        .in_tier(StorageTier::Instance)
        .iter()
        .any(|entry| entry.key.contains("Guardian")));
    assert!(storage
        .in_tier(StorageTier::Persistent)
        .iter()
        .any(|entry| entry.key.contains(&id.to_string())));
}

#[test]
fn storage_entries_match_the_core_tier_api() {
    let fixture = Fixture::initialized();
    let storage = StorageSnapshot::capture(&fixture.ctx.env, &fixture.contract_id);
    assert!(!storage.in_tier(CoreStorageTier::Instance).is_empty());
}

#[test]
fn proposal_snapshot_has_persistent_state() {
    let fixture = Fixture::funded();
    fixture.propose(100);
    let storage = StorageSnapshot::capture(&fixture.ctx.env, &fixture.contract_id);
    assert!(!storage.in_tier(StorageTier::Persistent).is_empty());
}

#[test]
fn budget_snapshot_diff_is_non_negative() {
    let fixture = Fixture::initialized();
    let before = snapshot(&fixture);
    fixture.fund(10);
    let after = snapshot(&fixture);
    let diff = before.diff(&after);
    assert!(after.cpu_insns >= before.cpu_insns);
    assert!(diff.mem_bytes <= after.mem_bytes);
}

#[test]
fn guardian_veto_does_not_change_treasury_balance() {
    let fixture = Fixture::funded();
    let balance = fixture.client().balance();
    let id = fixture.ready_proposal(300);
    fixture.client().veto(&fixture.guardian, &id);
    assert_eq!(fixture.client().balance(), balance);
}
