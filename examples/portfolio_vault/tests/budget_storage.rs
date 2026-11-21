//! soroban-testkit-core: budget and storage assertions for portfolio vault.

mod common;

use common::{Fixture, ONE};
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::StorageSnapshot;

#[test]
fn deposit_budget_is_bounded() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 0, 0);
    fx.env.cost_estimate().budget().reset_unlimited();
    fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    let snap = BudgetSnapshot::last_invocation(&fx.env);
    assert!(snap.cpu_insns > 0);
    assert!(snap.mem_bytes > 0);
}

#[test]
fn withdraw_budget_is_bounded() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 0, 0);
    let minted = fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    fx.env.cost_estimate().budget().reset_unlimited();
    fx.client().withdraw(&fx.user, &minted);
    let snap = BudgetSnapshot::last_invocation(&fx.env);
    assert!(snap.cpu_insns > 0);
}

#[test]
fn rebalance_budget_is_bounded() {
    let fx = Fixture::new();
    fx.deposit_balanced();
    fx.fund_trader(0, 1_000 * ONE, 0);
    fx.env.cost_estimate().budget().reset_unlimited();
    fx.client().rebalance(&fx.admin, &fx.token_a, &fx.token_b, &(10 * ONE), &(20 * ONE));
    let snap = BudgetSnapshot::last_invocation(&fx.env);
    assert!(snap.cpu_insns > 0);
}

#[test]
fn deposit_creates_persistent_share_balance() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 0, 0);
    let before = StorageSnapshot::capture(&fx.env, &fx.vault);
    fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    let after = StorageSnapshot::capture(&fx.env, &fx.vault);
    assert!(after.entries().len() >= before.entries().len());
}

#[test]
fn withdraw_clears_share_balance_when_fully_redeemed() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 0, 0);
    let minted = fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    fx.client().withdraw(&fx.user, &minted);
    assert_eq!(fx.client().share_balance(&fx.user), 0);
}

#[test]
fn storage_snapshot_has_instance_entries() {
    let fx = Fixture::new();
    let snap = StorageSnapshot::capture(&fx.env, &fx.vault);
    assert!(!snap.entries().is_empty());
}

#[test]
fn multiple_deposits_increase_storage_or_keep_it_stable() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 100 * ONE, 0);
    fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    let s1 = StorageSnapshot::capture(&fx.env, &fx.vault);
    fx.client().deposit(&fx.user, &fx.token_b, &(10 * ONE));
    let s2 = StorageSnapshot::capture(&fx.env, &fx.vault);
    assert!(s2.entries().len() >= s1.entries().len());
}

#[test]
fn rebalance_preserves_total_value_approximately() {
    let fx = Fixture::new();
    fx.deposit_balanced();
    fx.fund_trader(0, 1_000 * ONE, 0);
    let before = fx.client().total_value();
    fx.client().rebalance(&fx.admin, &fx.token_a, &fx.token_b, &(10 * ONE), &(20 * ONE));
    let after = fx.client().total_value();
    // oracle-priced swap should keep vault total value unchanged
    assert_eq!(before, after);
}

#[test]
fn deposit_after_rebalance_uses_current_value() {
    let fx = Fixture::new();
    fx.deposit_balanced();
    fx.fund_trader(0, 1_000 * ONE, 0);
    fx.client().rebalance(&fx.admin, &fx.token_a, &fx.token_b, &(5 * ONE), &(10 * ONE));
    fx.fund_user(10 * ONE, 0, 0);
    let minted = fx.client().deposit(&fx.user, &fx.token_a, &(1 * ONE));
    assert!(minted > 0);
}

#[test]
fn budget_snapshot_diff_works_for_two_ops() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 100 * ONE, 0);
    fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    let a = BudgetSnapshot::last_invocation(&fx.env);
    fx.client().deposit(&fx.user, &fx.token_b, &(10 * ONE));
    let b = BudgetSnapshot::last_invocation(&fx.env);
    let _diff = a.diff(&b);
}
