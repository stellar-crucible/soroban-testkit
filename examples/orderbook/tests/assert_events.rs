extern crate std;

use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::Address;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{
    assert_auth_matches, assert_event_count_range, assert_event_topic_count,
};
use soroban_testkit_example_orderbook::{
    OrderBookContract, OrderBookContractClient, Side, PRICE_PRECISION,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const ONE: i128 = 10_000_000;

struct OrderBookAssertFixture {
    ctx: TestContext,
    book: Address,
    _base_token: Address,
    _quote_token: Address,
    maker: Address,
    taker: Address,
}

fn setup() -> OrderBookAssertFixture {
    let ctx = TestContextBuilder::new().with_users(2).build();

    let book = ctx.env.register(OrderBookContract, ());
    let base_token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    let quote_token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let maker = ctx.user(0).clone();
    let taker = ctx.user(1).clone();

    for user in [&maker, &taker] {
        StellarAssetClient::new(&ctx.env, &base_token).mint(user, &(1_000_000 * ONE));
        StellarAssetClient::new(&ctx.env, &quote_token).mint(user, &(1_000_000 * ONE));
    }

    let client = OrderBookContractClient::new(&ctx.env, &book);
    client.initialize(&ctx.admin, &base_token, &quote_token, &10, &20);

    OrderBookAssertFixture {
        ctx,
        book,
        _base_token: base_token,
        _quote_token: quote_token,
        maker,
        taker,
    }
}

#[test]
fn assert_order_placed_event_emitted_with_expected_topics() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &(100 * ONE));

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.book)
        .with_topic("placed")
        .assert_emitted();

    assert_event_count_range!(fx.ctx.env, 1..=10);
    assert_event_topic_count!(fx.ctx.env, "placed", 1);
}

#[test]
fn assert_order_cancelled_event_emitted_on_cancellation() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let order_id = client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &(50 * ONE));

    client.cancel_order(&fx.maker, &order_id);

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.book)
        .with_topic("cancelled")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "cancelled", 1);
}

#[test]
fn assert_trade_event_emitted_on_order_matching() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    client.place_order(&fx.maker, &Side::Sell, &(3 * PRICE_PRECISION), &(50 * ONE));
    client.execute_market_order(&fx.taker, &Side::Buy, &(25 * ONE));

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.book)
        .with_topic("trade")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "trade", 1);
}

#[test]
fn assert_maker_auth_required_for_placing_order() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    client.place_order(&fx.maker, &Side::Sell, &(2 * PRICE_PRECISION), &(10 * ONE));

    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.maker,
        function: "place_order",
        args: [fx.maker.clone(), Side::Sell as u32, 2 * PRICE_PRECISION, 10 * ONE],
    });
}

#[test]
fn assert_maker_auth_required_for_cancelling_order() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let order_id = client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));
    client.cancel_order(&fx.maker, &order_id);

    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.maker,
        function: "cancel_order",
        args: [fx.maker.clone(), order_id],
    });
}

#[test]
fn assert_taker_auth_required_for_market_execution() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    client.place_order(&fx.maker, &Side::Sell, &(2 * PRICE_PRECISION), &(10 * ONE));
    client.execute_market_order(&fx.taker, &Side::Buy, &(10 * ONE));

    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.taker,
        function: "execute_market_order",
        args: [fx.taker.clone(), Side::Buy as u32, 10 * ONE],
    });
}

#[test]
fn assert_multiple_trades_accumulate_in_event_log() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let mut log = EventLog::new(&fx.ctx.env);

    client.place_order(&fx.maker, &Side::Sell, &(2 * PRICE_PRECISION), &(10 * ONE));
    client.place_order(&fx.maker, &Side::Sell, &(3 * PRICE_PRECISION), &(10 * ONE));

    client.execute_market_order(&fx.taker, &Side::Buy, &(20 * ONE));
    log.collect();

    log.matcher().with_topic("trade").assert_count(2);
    assert_event_topic_count!(log, "trade", 2);
}

#[test]
fn negative_assertion_proves_no_spurious_events() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    client.place_order(&fx.maker, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));

    EventMatcher::new(&fx.ctx.env)
        .with_topic("Liquidate")
        .assert_not_emitted();

    EventMatcher::new(&fx.ctx.env)
        .with_topic("Emergency")
        .assert_not_emitted();
}
