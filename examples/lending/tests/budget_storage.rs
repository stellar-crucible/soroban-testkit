//! Resource use and storage footprint measured with `soroban-testkit-core`:
//! ceilings on the deposit, borrow and liquidate hot paths, a committed
//! baseline with an explicit recorder, tier checks and storage diffs.
//!
//! The readings meter native test contracts, so they exclude Wasm instantiation
//! and are only comparable under the pinned toolchain and lockfile. They are
//! baselines for regression detection, not network fee estimates.

mod common;

use std::path::{Path, PathBuf};

use common::*;
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::assert_storage_tier;
use soroban_testkit_core::budget::{BudgetBaseline, BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_core::StorageFragmentationAnalyzer;

/// Generous per-transaction ceilings: catch an order-of-magnitude regression,
/// not the small variation between equivalent invocations.
const CPU_CEILING: u64 = 50_000_000;
const MEM_CEILING: u64 = 10_000_000;
/// Growth the committed baseline tolerates before a test fails.
const BASELINE_TOLERANCE_PERCENT: u64 = 10;

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

/// The three hot paths, each measured in a fresh environment with the same
/// scenario the recorder and the baseline test both use.
fn measure_cases() -> Vec<(&'static str, BudgetSnapshot)> {
    let lending = Lending::new();
    lending.vault().deposit(&lending.owner, &(10_000 * ONE));
    let deposit = BudgetSnapshot::last_invocation(&lending.ctx.env);

    let lending = Lending::new();
    lending.deposit(&lending.owner, 10_000 * ONE);
    lending.borrow(&lending.owner, 6_000 * ONE);
    let borrow = BudgetSnapshot::last_invocation(&lending.ctx.env);

    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.arm_liquidator(6_000 * ONE);
    lending.set_price(6_500_000);
    lending
        .pool()
        .liquidate(&lending.liquidator, &lending.owner);
    let liquidate = BudgetSnapshot::last_invocation(&lending.ctx.env);

    vec![
        ("deposit", deposit),
        ("borrow", borrow),
        ("liquidate", liquidate),
    ]
}

#[test]
fn every_hot_path_stays_inside_a_transaction_ceiling() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    let guard = |case: &str| {
        BudgetGuard::new(case)
            .cpu_ceiling(CPU_CEILING)
            .mem_ceiling(MEM_CEILING)
    };

    guard("deposit").run(env, || {
        lending.vault().deposit(&lending.owner, &(10_000 * ONE));
    });
    guard("borrow").run(env, || {
        lending.pool().borrow(&lending.owner, &(2_000 * ONE));
    });
    guard("accrue").run(env, || {
        lending.pool().accrue(&lending.owner);
    });
    guard("repay").run(env, || {
        lending.pool().repay(&lending.owner, &(1_000 * ONE));
    });
    guard("withdraw").run(env, || {
        lending.vault().withdraw(&lending.owner, &(1_000 * ONE));
    });
}

#[test]
fn committed_budget_baseline_covers_deposit_borrow_and_liquidate() {
    let committed = BudgetBaseline::load(&baseline_path())
        .expect("examples/lending/budget.json must exist and parse");
    assert_eq!(committed.len(), 3);

    for (case, cost) in measure_cases() {
        assert!(
            cost.cpu_insns > 0 && cost.mem_bytes > 0,
            "{case} was metered"
        );
        committed
            .guard(case)
            .cpu_ceiling(CPU_CEILING)
            .mem_ceiling(MEM_CEILING)
            .tolerance_percent(BASELINE_TOLERANCE_PERCENT)
            .assert_within(&cost);
    }
}

#[test]
fn a_view_costs_less_than_a_write() {
    let lending = Lending::new();
    let env = &lending.ctx.env;

    lending.vault().position(&lending.owner);
    let read = BudgetSnapshot::last_invocation(env);

    lending.vault().deposit(&lending.owner, &(10_000 * ONE));
    let write = BudgetSnapshot::last_invocation(env);

    assert!(read.cpu_insns > 0 && write.cpu_insns > 0);
    assert!(
        read.cpu_insns < write.cpu_insns,
        "read={read:?} write={write:?}"
    );
}

#[test]
fn a_budget_snapshot_diff_reports_the_more_expensive_call() {
    let cases = measure_cases();
    let liquidate = cases
        .iter()
        .find(|(case, _)| *case == "liquidate")
        .map(|(_, cost)| *cost)
        .unwrap();
    let deposit = cases
        .iter()
        .find(|(case, _)| *case == "deposit")
        .map(|(_, cost)| *cost)
        .unwrap();

    let diff = deposit.diff(&liquidate);
    assert!(
        diff.cpu_insns > 0,
        "liquidate should meter more work: {diff:?}"
    );
    assert!(
        diff.mem_bytes > 0,
        "liquidate should meter more bytes: {diff:?}"
    );
}

#[test]
fn a_deposit_adds_one_persistent_vault_entry() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    let before = StorageSnapshot::capture(env, &lending.vault_id);

    lending.vault().deposit(&lending.owner, &(1_000 * ONE));

    let after = StorageSnapshot::capture(env, &lending.vault_id);
    let diff = before.diff(&after);
    assert_eq!(diff.added().len(), 1, "{diff:?}");
    assert!(diff.removed().is_empty());
    assert_storage_tier!(
        env,
        &lending.vault_id,
        "[Position,*",
        StorageTier::Persistent
    );
    assert_storage_tier_count!(env, lending.vault_id, persistent = 1, temporary = 0);
}

#[test]
fn each_depositor_gets_a_separate_vault_entry() {
    let lending = Lending::new();
    let env = &lending.ctx.env;

    lending.deposit(&lending.owner, 10_000 * ONE);
    lending.deposit(&lending.other, 20_000 * ONE);

    assert_storage_tier_count!(env, lending.vault_id, persistent = 2);
    assert_eq!(lending.vault().total_collateral(), 30_000 * ONE);
    // The collateral token keeps one balance entry per depositor.
    assert_storage_tier_count!(env, lending.collateral, persistent = 4);
}

#[test]
fn a_second_borrow_rewrites_the_pool_position_in_place() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.deposit(&lending.owner, 10_000 * ONE);
    lending.borrow(&lending.owner, 2_000 * ONE);
    let before = StorageSnapshot::capture(env, &lending.pool_id);

    lending.borrow(&lending.owner, 1_000 * ONE);

    let diff = before.diff(&StorageSnapshot::capture(env, &lending.pool_id));
    assert!(diff.added().is_empty(), "{diff:?}");
    assert!(diff.removed().is_empty(), "{diff:?}");
    assert!(!diff.modified().is_empty(), "{diff:?}");
}

#[test]
fn repayment_and_withdrawal_release_the_position_entries() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    let pool = StorageSnapshot::capture(env, &lending.pool_id);
    let vault = StorageSnapshot::capture(env, &lending.vault_id);

    lending.pool().repay(&lending.owner, &(6_000 * ONE));
    lending.vault().withdraw(&lending.owner, &(10_000 * ONE));

    assert_eq!(
        pool.diff(&StorageSnapshot::capture(env, &lending.pool_id))
            .removed()
            .len(),
        1
    );
    assert_eq!(
        vault
            .diff(&StorageSnapshot::capture(env, &lending.vault_id))
            .removed()
            .len(),
        1
    );
    assert_storage_tier_count!(env, lending.pool_id, persistent = 0);
    assert_storage_tier_count!(env, lending.vault_id, persistent = 0);
    assert_storage_tier_count!(env, lending.debt_id, persistent = 0);
}

#[test]
fn liquidation_leaves_only_the_liquidators_entries() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.arm_liquidator(6_000 * ONE);
    // At $0.10 the seized amount exceeds the whole position.
    lending.set_price(1_000_000);

    lending
        .pool()
        .liquidate(&lending.liquidator, &lending.owner);

    assert_storage_tier_count!(env, lending.pool_id, persistent = 1);
    assert_storage_tier_count!(env, lending.vault_id, persistent = 1);
    assert_eq!(lending.pool().debt_of(&lending.owner), 0);
}

#[test]
fn a_fragmentation_report_covers_the_vault_storage() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.deposit(&lending.owner, 10_000 * ONE);
    lending.deposit(&lending.other, 20_000 * ONE);

    let report = StorageFragmentationAnalyzer::new()
        .with_overhead_threshold(0.5)
        .with_stroop_rate_per_byte(100)
        .analyze(env, &lending.vault_id);

    assert!(!report.entries.is_empty());
    assert!(report.total_bytes > 0);
    assert!(report.to_markdown().contains("## Tier Breakdown"));
}

#[test]
#[ignore = "rewrites examples/lending/budget.json — run it deliberately, then commit the file"]
fn budget_baseline_records_current_costs() {
    let mut baseline = BudgetBaseline::new();
    for (case, cost) in measure_cases() {
        baseline.record(case, cost);
    }
    baseline.save(&baseline_path()).unwrap();
    println!("{}", baseline.to_json_string());
}
