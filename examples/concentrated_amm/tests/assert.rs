extern crate std;

use soroban_sdk::{testutils::Address as _, Address};
use soroban_testkit_assert::assert_contract_error;
use soroban_testkit_example_concentrated_amm::{AmmError, ConcentratedAmm, ConcentratedAmmClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;

fn pool() -> (
    soroban_testkit_fixtures::TestContext,
    ConcentratedAmmClient<'static>,
) {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let contract = ctx.env.register(ConcentratedAmm, ());
    let client = ConcentratedAmmClient::new(&ctx.env, &contract);
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1_000_000,
        &30,
    );
    client.mint_position(&ctx.user(0), &-600, &600, &10_000);
    (ctx, client)
}

#[test]
fn swap_zero_for_one_changes_reserves() {
    let (ctx, c) = pool();
    let out = c.swap(&ctx.user(1), &100, &true);
    assert!(out > 0);
    assert_eq!(c.get_tick(), -60);
}
#[test]
fn swap_one_for_zero_crosses_upward() {
    let (ctx, c) = pool();
    c.swap(&ctx.user(1), &100, &true);
    c.swap(&ctx.user(1), &100, &false);
    assert!(c.get_tick() > -60);
}
#[test]
fn fee_is_applied() {
    let (ctx, c) = pool();
    let out = c.swap(&ctx.user(1), &100, &true);
    assert!(out > 0);
    assert!(c.get_reserves().1 < 100_000);
}
#[test]
fn zero_swap_rejected() {
    let (ctx, c) = pool();
    assert_contract_error!(c.try_swap(&ctx.user(1), &0, &true), AmmError::InvalidAmount);
}
#[test]
fn negative_swap_rejected() {
    let (ctx, c) = pool();
    assert_eq!(
        c.try_swap(&ctx.user(1), &-1, &false).unwrap_err().unwrap(),
        AmmError::InvalidAmount
    );
}
#[test]
fn unauthorized_burn_rejected() {
    let (ctx, c) = pool();
    assert_eq!(
        c.try_burn_position(&ctx.user(1), &0).unwrap_err().unwrap(),
        AmmError::Unauthorized
    );
}
#[test]
fn owner_burn_removes_liquidity() {
    let (ctx, c) = pool();
    let burned = c.burn_position(&ctx.user(0), &0);
    assert_eq!(burned, 10_000);
    assert_eq!(c.get_liquidity(), 0);
}
#[test]
fn missing_position_rejected() {
    let (ctx, c) = pool();
    assert_eq!(
        c.try_burn_position(&ctx.user(0), &99).unwrap_err().unwrap(),
        AmmError::PositionNotFound
    );
}
