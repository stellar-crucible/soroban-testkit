//! Cost and footprint of the protocol, measured with `soroban-testkit-core` and
//! checked with `soroban-testkit-assert`: liquidation gas under collateral
//! drops of increasing severity, budget ceilings on every user-facing call,
//! and how positions and balances occupy and release storage.

mod common;

use common::*;
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_core::StorageFragmentationAnalyzer;

/// Generous ceilings for one liquidation: three contracts, a token transfer,
/// a burn, and the oracle read. They catch a regression of an order of
/// magnitude, not noise.
const LIQUIDATION_CPU_CEILING: u64 = 20_000_000;
const LIQUIDATION_MEM_CEILING: u64 = 5_000_000;

/// One row of the liquidation benchmark.
struct LiquidationCost {
    drop_percent: i128,
    underwater: bool,
    seized: i128,
    remaining: i128,
    cost: BudgetSnapshot,
}

/// Opens a position at 160% (just above the 150% MCR), crashes the price by
/// `drop_percent`, and liquidates it under a budget guard.
fn liquidate_after_drop(drop_percent: i128) -> LiquidationCost {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    // $1,000 of collateral, 625 sUSD of debt = 160%.
    cdp.open(&cdp.owner, 10_000 * ONE, 625 * ONE);
    cdp.arm_liquidator(625 * ONE);
    cdp.crash_price(drop_percent);
    assert!(cdp.engine().is_liquidatable(&cdp.owner));

    let guard = BudgetGuard::new(format!("liquidate_after_{drop_percent}pct_drop"))
        .cpu_ceiling(LIQUIDATION_CPU_CEILING)
        .mem_ceiling(LIQUIDATION_MEM_CEILING);
    let result = guard.run(env, || cdp.engine().liquidate(&cdp.liquidator, &cdp.owner));
    let cost = BudgetSnapshot::last_invocation(env);

    LiquidationCost {
        drop_percent,
        underwater: result.collateral_remaining == 0,
        seized: result.collateral_seized,
        remaining: result.collateral_remaining,
        cost,
    }
}

#[test]
fn liquidation_gas_across_collateral_drop_severities() {
    // 10%: just under the MCR. 30%: still solvent, but debt plus the bonus
    // takes almost every unit. 45%, 60%, 90%: collateral is worth less than
    // the debt, so the liquidator takes all of it and the position is closed.
    let severities = [10, 30, 45, 60, 90];
    let rows: Vec<LiquidationCost> = severities
        .iter()
        .map(|&drop| liquidate_after_drop(drop))
        .collect();

    println!(
        "| Price drop | Underwater | Seized (XLM) | Remaining (XLM) | CPU insns | Mem bytes |"
    );
    println!(
        "|-----------:|:----------:|-------------:|----------------:|----------:|----------:|"
    );
    for row in &rows {
        println!(
            "| {:>9}% | {:^10} | {:>12} | {:>15} | {:>9} | {:>9} |",
            row.drop_percent,
            if row.underwater { "yes" } else { "no" },
            row.seized / ONE,
            row.remaining / ONE,
            row.cost.cpu_insns,
            row.cost.mem_bytes,
        );
    }

    for row in &rows {
        assert!(row.cost.cpu_insns > 0 && row.cost.mem_bytes > 0);
        assert!(row.cost.cpu_insns <= LIQUIDATION_CPU_CEILING);
        assert!(row.cost.mem_bytes <= LIQUIDATION_MEM_CEILING);
    }

    // Deeper drops seize more collateral until the position is exhausted.
    for pair in rows.windows(2) {
        assert!(pair[1].seized >= pair[0].seized);
    }
    assert!(!rows[0].underwater && !rows[1].underwater);
    assert!(rows[3].underwater && rows[4].underwater);

    // Severity changes which branch runs, not how much work the contract
    // does: the most and least expensive cases stay within 2x of each other.
    let cpu: Vec<u64> = rows.iter().map(|row| row.cost.cpu_insns).collect();
    let (min, max) = (cpu.iter().min().unwrap(), cpu.iter().max().unwrap());
    assert!(
        *max <= *min * 2,
        "liquidation cost varies too much with severity: {cpu:?}"
    );
}

#[test]
fn every_user_facing_call_fits_its_budget() {
    let mut cdp = Cdp::new();
    let env = cdp.ctx.env.clone();
    let guard = |case: &str| {
        BudgetGuard::new(case)
            .cpu_ceiling(15_000_000)
            .mem_ceiling(4_000_000)
    };

    guard("deposit").run(&env, || cdp.engine().deposit(&cdp.owner, &(10_000 * ONE)));
    guard("borrow").run(&env, || cdp.engine().borrow(&cdp.owner, &(500 * ONE)));
    cdp.advance_time(YEAR);
    guard("accrue").run(&env, || cdp.engine().accrue(&cdp.owner));
    guard("repay").run(&env, || cdp.engine().repay(&cdp.owner, &(100 * ONE)));
    guard("withdraw").run(&env, || cdp.engine().withdraw(&cdp.owner, &(1_000 * ONE)));
}

#[test]
fn opening_a_position_adds_exactly_one_persistent_entry() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    let before = StorageSnapshot::capture(env, &cdp.engine_id);

    cdp.engine().deposit(&cdp.owner, &(1_000 * ONE));

    let diff = before.diff(&StorageSnapshot::capture(env, &cdp.engine_id));
    assert_eq!(diff.added().len(), 1, "added: {:?}", diff.added());
    assert_storage_tier_count!(env, cdp.engine_id, {
        max_instance: 8,
        persistent: 1,
        temporary: 0,
    });
}

#[test]
fn each_owner_gets_their_own_position_entry() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;

    cdp.open(&cdp.owner, 10_000 * ONE, 100 * ONE);
    cdp.open(&cdp.other, 10_000 * ONE, 100 * ONE);

    assert_storage_tier_count!(env, cdp.engine_id, persistent = 2, temporary = 0);
    // The token keeps one balance entry per holder.
    assert_storage_tier_count!(env, cdp.synth_id, persistent = 2);
}

#[test]
fn closing_a_position_releases_its_storage() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.open(&cdp.owner, 10_000 * ONE, 300 * ONE);
    let before = StorageSnapshot::capture(env, &cdp.engine_id);

    cdp.engine().repay(&cdp.owner, &(300 * ONE));
    cdp.engine().withdraw(&cdp.owner, &(10_000 * ONE));

    let diff = before.diff(&StorageSnapshot::capture(env, &cdp.engine_id));
    assert_eq!(diff.removed().len(), 1);
    assert_storage_tier_count!(env, cdp.engine_id, persistent = 0);
    // A zero balance is deleted rather than stored.
    assert_storage_tier_count!(env, cdp.synth_id, persistent = 0);
}

#[test]
fn an_underwater_liquidation_removes_the_emptied_position() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(600 * ONE);
    cdp.crash_price(60);

    cdp.engine().liquidate(&cdp.liquidator, &cdp.owner);

    // Only the liquidator's own position remains.
    assert_storage_tier_count!(env, cdp.engine_id, persistent = 1);
}

#[test]
fn borrowing_rewrites_the_position_in_place() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));
    let before = StorageSnapshot::capture(env, &cdp.engine_id);

    cdp.engine().borrow(&cdp.owner, &(100 * ONE));

    let diff = before.diff(&StorageSnapshot::capture(env, &cdp.engine_id));
    assert!(diff.added().is_empty());
    assert!(diff.removed().is_empty());
    // The position and the instance totals change.
    assert!(!diff.modified().is_empty());
}

#[test]
fn fragmentation_report_covers_the_engine_storage() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.open(&cdp.owner, 10_000 * ONE, 100 * ONE);
    cdp.open(&cdp.other, 20_000 * ONE, 200 * ONE);

    let report = StorageFragmentationAnalyzer::new()
        .with_overhead_threshold(0.5)
        .with_stroop_rate_per_byte(100)
        .analyze(env, &cdp.engine_id);

    assert!(!report.entries.is_empty());
    assert!(report.total_bytes > 0);
    assert!(report.to_markdown().contains("## Tier Breakdown"));
}
