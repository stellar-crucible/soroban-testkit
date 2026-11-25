extern crate std;

use soroban_sdk::{testutils::Address as _, Address};
use soroban_testkit_example_concentrated_amm::*;
use soroban_testkit_fixtures::builder::TestContextBuilder;

fn setup() -> (
    soroban_testkit_fixtures::TestContext,
    Address,
    ConcentratedAmmClient<'static>,
) {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let contract = ctx.env.register(ConcentratedAmm, ());
    let client = ConcentratedAmmClient::new(&ctx.env, &contract);
    (ctx, contract, client)
}

#[test]
fn fixture_initializes_demo_pool() {
    let (ctx, _contract, client) = setup();
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1_000_000,
        &30,
    );
    assert_eq!(client.get_liquidity(), 0);
    assert_eq!(client.get_reserves(), (0, 0));
}
#[test]
fn fixture_rejects_double_initialization() {
    let (ctx, _, client) = setup();
    let a = Address::generate(&ctx.env);
    let b = Address::generate(&ctx.env);
    client.initialize(&ctx.admin, &a, &b, &1_000_000, &30);
    assert!(client
        .try_initialize(&ctx.admin, &a, &b, &1_000_000, &30)
        .is_err());
}
#[test]
fn fixture_rejects_uninitialized_mint() {
    let (ctx, _, client) = setup();
    assert_eq!(
        client
            .try_mint_position(ctx.user(0), &-120, &120, &100)
            .unwrap_err()
            .unwrap(),
        AmmError::NotInitialized
    );
}
#[test]
fn fixture_mints_valid_range() {
    let (ctx, _, client) = setup();
    let a = Address::generate(&ctx.env);
    let b = Address::generate(&ctx.env);
    client.initialize(&ctx.admin, &a, &b, &1_000_000, &30);
    let id = client.mint_position(ctx.user(0), &-120, &120, &100);
    assert_eq!(id, 0);
    assert_eq!(client.get_liquidity(), 100);
}
#[test]
fn fixture_rejects_inverted_ticks() {
    let (ctx, _, client) = setup();
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1,
        &30,
    );
    assert_eq!(
        client
            .try_mint_position(ctx.user(0), &120, &-120, &100)
            .unwrap_err()
            .unwrap(),
        AmmError::InvalidTick
    );
}
#[test]
fn fixture_rejects_unaligned_ticks() {
    let (ctx, _, client) = setup();
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1,
        &30,
    );
    assert_eq!(
        client
            .try_mint_position(ctx.user(0), &-61, &120, &100)
            .unwrap_err()
            .unwrap(),
        AmmError::InvalidTickSpacing
    );
}
#[test]
fn fixture_rejects_zero_liquidity() {
    let (ctx, _, client) = setup();
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1,
        &30,
    );
    assert_eq!(
        client
            .try_mint_position(ctx.user(0), &-120, &120, &0)
            .unwrap_err()
            .unwrap(),
        AmmError::InvalidAmount
    );
}
#[test]
fn fixture_position_round_trip() {
    let (ctx, _, client) = setup();
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1,
        &30,
    );
    let id = client.mint_position(ctx.user(0), &-120, &120, &250);
    let position = client.get_position(&id).unwrap();
    assert_eq!(position.owner, ctx.user(0).clone());
    assert_eq!(position.liquidity, 250);
}
