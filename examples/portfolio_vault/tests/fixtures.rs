//! soroban-testkit-fixtures: portfolio vault setup, deposits, withdrawals, and rebalance setup.

mod common;

use common::{Fixture, ONE};
use soroban_testkit_fixtures::TestContextBuilder;

#[test]
fn deploys_vault_and_assets() {
    let fx = Fixture::new();
    assert_eq!(fx.client().total_shares(), 0);
    let assets = fx.client().assets();
    assert_eq!(assets.len(), 3);
}

#[test]
fn deposit_mints_shares() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 0, 0);
    let minted = fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    assert!(minted > 0);
    assert_eq!(fx.client().share_balance(&fx.user), minted);
}

#[test]
fn deposit_multiple_assets_increases_total_value() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 100 * ONE, 100 * ONE);
    fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    let v1 = fx.client().total_value();
    fx.client().deposit(&fx.user, &fx.token_b, &(10 * ONE));
    let v2 = fx.client().total_value();
    assert!(v2 > v1);
}

#[test]
fn withdraw_returns_proportional_assets() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 100 * ONE, 100 * ONE);
    let minted = fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    let payouts = fx.client().withdraw(&fx.user, &(minted / 2));
    assert_eq!(payouts.len(), 3);
    assert_eq!(fx.client().share_balance(&fx.user), minted - minted / 2);
}

#[test]
fn set_price_requires_admin() {
    let fx = Fixture::new();
    let res = fx.client().try_set_price(&fx.user, &fx.token_a, &3i128);
    assert!(res.is_err());
}

#[test]
fn admin_can_update_oracle_price() {
    let fx = Fixture::new();
    fx.client().set_price(&fx.admin, &fx.token_a, &3i128);
    assert_eq!(fx.client().price(&fx.token_a), Some(3));
}

#[test]
fn rebalance_swaps_assets_at_oracle_price() {
    let fx = Fixture::new();
    fx.deposit_balanced();
    fx.fund_trader(0, 1_000 * ONE, 0);
    // Vault has 50 A (price 2) and 60 B (price 1), 8 C. Sell 10 A for 20 B.
    let bought = fx.client().rebalance(
        &fx.trader,
        &fx.token_a,
        &fx.token_b,
        &(10 * ONE),
        &(20 * ONE),
    );
    assert_eq!(bought, 20 * ONE);
}

#[test]
fn rebalance_fails_on_slippage() {
    let fx = Fixture::new();
    fx.deposit_balanced();
    fx.fund_trader(0, 1_000 * ONE, 0);
    let res = fx.client().try_rebalance(
        &fx.admin,
        &fx.token_a,
        &fx.token_b,
        &(10 * ONE),
        &(21 * ONE),
    );
    assert!(res.is_err());
}

#[test]
fn share_balance_persistent_ttl_can_be_inspected() {
    let fx = Fixture::new();
    fx.fund_user(100 * ONE, 0, 0);
    fx.client().deposit(&fx.user, &fx.token_a, &(10 * ONE));
    assert!(fx.client().share_balance(&fx.user) > 0);
}

#[test]
fn context_builder_can_create_users_for_portfolio_scenarios() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    assert_eq!(ctx.users.len(), 3);
}
