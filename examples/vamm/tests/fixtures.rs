extern crate std;

use soroban_sdk::Address;
use soroban_testkit_example_vamm::{VammContract, VammContractClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;

fn deploy(ctx: &soroban_testkit_fixtures::TestContext) -> (Address, VammContractClient<'_>) {
    let contract = ctx.env.register(VammContract, ());
    let client = VammContractClient::new(&ctx.env, &contract);
    client.initialize(&10_000_000, &10_000_000);
    (contract, client)
}

#[test]
fn builder_provides_distinct_trader_and_counterparty() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let (_, _) = deploy(&ctx);
    assert_eq!(ctx.users.len(), 3);
    assert_ne!(ctx.user(0), ctx.user(1));
}

#[test]
fn initialization_pins_a_one_to_one_mark() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_, client) = deploy(&ctx);
    assert_eq!(client.mark(), 10_000_000);
    let position = client.open_position(ctx.user(0), &1, &1_000, &2);
    assert_eq!(position.entry_mark, 10_000_000);
}

#[test]
fn opens_a_leveraged_long() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_, client) = deploy(&ctx);
    let position = client.open_position(ctx.user(0), &1, &1_000, &5);
    assert_eq!(position.size, 5_000);
}

#[test]
fn opens_a_leveraged_short() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_, client) = deploy(&ctx);
    let position = client.open_position(ctx.user(0), &-1, &1_000, &3);
    assert_eq!(position.size, -3_000);
}

#[test]
fn rejects_a_second_position_for_the_same_trader() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_, client) = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    assert!(client.try_open_position(ctx.user(0), &1, &500, &2).is_err());
}

#[test]
fn rejects_invalid_side() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_, client) = deploy(&ctx);
    assert!(client
        .try_open_position(ctx.user(0), &0, &1_000, &2)
        .is_err());
}

#[test]
fn rejects_zero_collateral_and_excessive_leverage() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let (_, client) = deploy(&ctx);
    assert!(client.try_open_position(ctx.user(0), &1, &0, &2).is_err());
    assert!(client
        .try_open_position(ctx.user(1), &1, &1_000, &11)
        .is_err());
}

#[test]
fn ledger_clock_advances_without_changing_positions() {
    let mut ctx = TestContextBuilder::new().with_users(2).build();
    ctx.set_timestamp(1_700_000_000);
    let contract = ctx.env.register(VammContract, ());
    let client = VammContractClient::new(&ctx.env, &contract);
    client.initialize(&10_000_000, &10_000_000);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    ctx.advance_time(86_400);
    assert!(client.position(ctx.user(0)).is_some());
}
