extern crate std;

use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_example_vamm::{VammContract, VammContractClient};

fn deploy(ctx: &soroban_testkit_fixtures::TestContext) -> VammContractClient<'_> {
    let contract = ctx.env.register(VammContract, ());
    let client = VammContractClient::new(&ctx.env, &contract);
    client.initialize(&10_000_000, &10_000_000);
    client
}

#[test]
fn open_requires_trader_authorization() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    assert_auth_matches!(&ctx.env, {
        invoker: ctx.user(0),
        function: "open_position",
    });
}

#[test]
fn close_requires_trader_authorization() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    client.close_position(ctx.user(0));
    assert_auth_matches!(&ctx.env, {
        invoker: ctx.user(0),
        function: "close_position",
    });
}

#[test]
fn add_margin_requires_trader_authorization() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    client.add_margin(ctx.user(0), &500);
    assert_auth_matches!(&ctx.env, {
        invoker: ctx.user(0),
        function: "add_margin",
    });
}

#[test]
fn auth_matcher_accepts_a_trader_address() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    AuthMatcher::new(&ctx.env)
        .invoker(ctx.user(0))
        .function("open_position")
        .assert_matches();
}

#[test]
fn long_profit_is_positive_after_an_upward_mark_move() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    client.move_mark(&10_000_000, &12_000_000);
    assert!(client.unrealized_pnl(ctx.user(0)) > 0);
}

#[test]
fn short_profit_is_positive_after_a_downward_mark_move() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &-1, &1_000, &2);
    client.move_mark(&10_000_000, &8_000_000);
    assert!(client.unrealized_pnl(ctx.user(0)) > 0);
}

#[test]
fn close_returns_collateral_plus_profit() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    client.move_mark(&10_000_000, &12_000_000);
    let settlement = client.close_position(ctx.user(0));
    assert!(settlement >= 1_000);
}

#[test]
fn close_removes_the_position() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let client = deploy(&ctx);
    client.open_position(ctx.user(0), &1, &1_000, &2);
    client.close_position(ctx.user(0));
    assert!(client.position(ctx.user(0)).is_none());
}
