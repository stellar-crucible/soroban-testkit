//! Cost and footprint of the pool, measured with `soroban-testkit-core` and
//! checked with `soroban-testkit-assert`: a budget ceiling for every
//! user-facing call, and how shares, policies, claims and votes occupy and
//! release storage.

mod common;

use common::*;
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::BudgetGuard;
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_core::StorageFragmentationAnalyzer;

/// Generous ceilings for one call: a token transfer, a handful of storage
/// writes and an event. They catch a regression of an order of magnitude, not
/// noise.
const CPU_CEILING: u64 = 50_000_000;
const MEM_CEILING: u64 = 10_000_000;

#[test]
fn every_user_facing_call_fits_its_budget() {
    let insurance = Insurance::new();
    let env = insurance.ctx.env.clone();
    let guard = |case: &str| {
        BudgetGuard::new(case)
            .cpu_ceiling(CPU_CEILING)
            .mem_ceiling(MEM_CEILING)
    };

    let shares = guard("deposit")
        .run(&env, || insurance.deposit(&insurance.provider, 1_000_000 * ONE));
    let policy = guard("buy_policy")
        .run(&env, || insurance.buy(&insurance.holder, 10_000 * ONE, YEAR));
    let claim = guard("file_claim")
        .run(&env, || insurance.file(&insurance.holder, policy.id, 8_000 * ONE));
    guard("vote").run(&env, || {
        insurance.cast(&insurance.assessors[0], claim.id, true)
    });
    guard("vote").run(&env, || {
        insurance.cast(&insurance.assessors[1], claim.id, true)
    });
    guard("settle").run(&env, || insurance.settle(claim.id));
    guard("withdraw").run(&env, || {
        insurance.pool().withdraw(&insurance.provider, &(shares / 2))
    });
}

#[test]
fn a_premium_and_a_settlement_each_fit_their_budget() {
    // A payout and a premium payment each move one token transfer and write
    // one entry, so neither should cost an order of magnitude more than the
    // other.
    let insurance = Insurance::new();
    let env = insurance.ctx.env.clone();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    let claim = insurance.file(&insurance.holder, policy.id, 8_000 * ONE);
    insurance.approve(claim.id);

    let premium = BudgetGuard::new("buy_policy")
        .cpu_ceiling(CPU_CEILING)
        .mem_ceiling(MEM_CEILING)
        .run(&env, || insurance.buy(&insurance.holder2, 10_000 * ONE, YEAR));
    let settlement = BudgetGuard::new("settle")
        .cpu_ceiling(CPU_CEILING)
        .mem_ceiling(MEM_CEILING)
        .run(&env, || insurance.settle(claim.id));

    assert!(premium.premium_paid > 0);
    assert!(settlement.payout > 0);
}

#[test]
fn a_deposit_adds_exactly_one_persistent_entry() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let before = StorageSnapshot::capture(env, &insurance.pool_id);

    insurance.deposit(&insurance.provider, 1_000 * ONE);

    let diff = before.diff(&StorageSnapshot::capture(env, &insurance.pool_id));
    assert_eq!(diff.added().len(), 1, "added: {:?}", diff.added());
    assert!(diff.removed().is_empty());
    assert_storage_tier_count!(env, insurance.pool_id, {
        max_instance: 12,
        persistent: 1,
        temporary: 0,
    });
}

#[test]
fn each_provider_owns_one_share_entry() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;

    insurance.deposit(&insurance.provider, 1_000 * ONE);
    insurance.deposit(&insurance.provider2, 2_000 * ONE);

    assert_storage_tier_count!(env, insurance.pool_id, persistent = 2, temporary = 0);
}

#[test]
fn a_policy_and_its_claim_are_separate_entries() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;

    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    assert_storage_tier_count!(env, insurance.pool_id, persistent = 1);

    insurance.file(&insurance.holder, policy.id, 8_000 * ONE);
    assert_storage_tier_count!(env, insurance.pool_id, persistent = 2);
}

#[test]
fn every_assessor_vote_is_recorded_once() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);

    // The policy and the claim, plus one entry per vote taken so far.
    assert_storage_tier_count!(env, insurance.pool_id, persistent = 2);
    insurance.cast(&insurance.assessors[0], claim.id, true);
    assert_storage_tier_count!(env, insurance.pool_id, persistent = 3);
    insurance.cast(&insurance.assessors[1], claim.id, true);
    assert_storage_tier_count!(env, insurance.pool_id, persistent = 4);
}

#[test]
fn withdrawing_every_share_releases_its_entry() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let shares = insurance.deposit(&insurance.provider, 1_000 * ONE);
    let before = StorageSnapshot::capture(env, &insurance.pool_id);

    insurance.pool().withdraw(&insurance.provider, &shares);

    let diff = before.diff(&StorageSnapshot::capture(env, &insurance.pool_id));
    assert_eq!(diff.removed().len(), 1);
    assert!(diff.added().is_empty());
    assert_storage_tier_count!(env, insurance.pool_id, persistent = 0);
}

#[test]
fn voting_rewrites_the_claim_in_place() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.cast(&insurance.assessors[0], claim.id, true);
    let before = StorageSnapshot::capture(env, &insurance.pool_id);

    insurance.cast(&insurance.assessors[1], claim.id, true);

    let diff = before.diff(&StorageSnapshot::capture(env, &insurance.pool_id));
    // Only the new vote entry appears: the claim is rewritten, not re-added.
    assert_eq!(diff.added().len(), 1);
    assert!(diff.removed().is_empty());
    assert!(!diff.modified().is_empty());
}

#[test]
fn settling_rewrites_the_claim_in_place() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.approve(claim.id);
    let before = StorageSnapshot::capture(env, &insurance.pool_id);

    insurance.settle(claim.id);

    let diff = before.diff(&StorageSnapshot::capture(env, &insurance.pool_id));
    assert!(diff.added().is_empty());
    assert!(diff.removed().is_empty());
    assert!(!diff.modified().is_empty());
}

#[test]
fn fragmentation_report_covers_the_pool_storage() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    insurance.deposit(&insurance.provider, 1_000 * ONE);
    insurance.deposit(&insurance.provider2, 2_000 * ONE);
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.cast(&insurance.assessors[0], claim.id, true);

    let report = StorageFragmentationAnalyzer::new()
        .with_overhead_threshold(0.5)
        .with_stroop_rate_per_byte(100)
        .analyze(env, &insurance.pool_id);

    assert!(!report.entries.is_empty());
    assert!(report.total_bytes > 0);
    assert!(report.to_markdown().contains("## Tier Breakdown"));
}
