extern crate std;

use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::Address;
use soroban_testkit_example_orderbook::{
    OrderBookContract, OrderBookContractClient, Side, PRICE_PRECISION,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const ONE: i128 = 10_000_000;

struct OrderBookFixture {
    ctx: TestContext,
    book: Address,
    base_token: Address,
    quote_token: Address,
    maker_1: Address,
    maker_2: Address,
    taker: Address,
}

fn setup() -> OrderBookFixture {
    let ctx = TestContextBuilder::new().with_users(3).build();

    let book = ctx.env.register(OrderBookContract, ());
    let base_token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    let quote_token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let maker_1 = ctx.user(0).clone();
    let maker_2 = ctx.user(1).clone();
    let taker = ctx.user(2).clone();

    for user in [&maker_1, &maker_2, &taker] {
        StellarAssetClient::new(&ctx.env, &base_token).mint(user, &(1_000_000 * ONE));
        StellarAssetClient::new(&ctx.env, &quote_token).mint(user, &(1_000_000 * ONE));
    }

    let client = OrderBookContractClient::new(&ctx.env, &book);
    client.initialize(&ctx.admin, &base_token, &quote_token, &10, &20);

    OrderBookFixture {
        ctx,
        book,
        base_token,
        quote_token,
        maker_1,
        maker_2,
        taker,
    }
}

#[test]
fn initialize_configures_tokens_and_empty_books() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let config = client.get_config();
    assert_eq!(config.admin, fx.ctx.admin);
    assert_eq!(config.base_token, fx.base_token);
    assert_eq!(config.quote_token, fx.quote_token);
    assert_eq!(config.next_order_id, 1);
    assert_eq!(client.get_book_depth(&Side::Buy), 0);
    assert_eq!(client.get_book_depth(&Side::Sell), 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);
    let err = client.try_initialize(&fx.ctx.admin, &fx.base_token, &fx.quote_token, &10, &20);
    assert!(err.is_err());
}

#[test]
fn place_buy_order_escrows_quote_funds_and_inserts_into_bids() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let quote_before = StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.maker_1);
    let order_id = client.place_order(
        &fx.maker_1,
        &Side::Buy,
        &(2 * PRICE_PRECISION),
        &(100 * ONE),
    );
    assert_eq!(order_id, 1);

    let quote_after = StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.maker_1);
    // Required quote = 100 * 2 = 200 ONE
    assert_eq!(quote_before - quote_after, 200 * ONE);

    let bids = client.get_bids();
    assert_eq!(bids.len(), 1);
    assert_eq!(bids.get(0).unwrap(), 1);

    let order = client.get_order(&order_id).unwrap();
    assert_eq!(order.maker, fx.maker_1);
    assert_eq!(order.side, Side::Buy);
    assert_eq!(order.price, 2 * PRICE_PRECISION);
    assert_eq!(order.amount, 100 * ONE);
    assert_eq!(order.filled, 0);
}

#[test]
fn place_sell_order_escrows_base_funds_and_inserts_into_asks() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let base_before = StellarAssetClient::new(&fx.ctx.env, &fx.base_token).balance(&fx.maker_1);
    let order_id = client.place_order(
        &fx.maker_1,
        &Side::Sell,
        &(3 * PRICE_PRECISION),
        &(50 * ONE),
    );
    assert_eq!(order_id, 1);

    let base_after = StellarAssetClient::new(&fx.ctx.env, &fx.base_token).balance(&fx.maker_1);
    assert_eq!(base_before - base_after, 50 * ONE);

    let asks = client.get_asks();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks.get(0).unwrap(), 1);
}

#[test]
fn place_orders_maintains_bid_price_priority_descending() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    // Place bid at 2.0
    let id1 = client.place_order(&fx.maker_1, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));
    // Place bid at 3.0 (higher price, must jump to front)
    let id2 = client.place_order(&fx.maker_1, &Side::Buy, &(3 * PRICE_PRECISION), &(10 * ONE));
    // Place bid at 2.5 (between 3.0 and 2.0)
    let id3 = client.place_order(
        &fx.maker_2,
        &Side::Buy,
        &(25 * PRICE_PRECISION / 10),
        &(10 * ONE),
    );

    let bids = client.get_bids();
    assert_eq!(bids.len(), 3);
    assert_eq!(bids.get(0).unwrap(), id2); // 3.0
    assert_eq!(bids.get(1).unwrap(), id3); // 2.5
    assert_eq!(bids.get(2).unwrap(), id1); // 2.0
}

#[test]
fn place_orders_maintains_ask_price_priority_ascending() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    // Place ask at 4.0
    let id1 = client.place_order(
        &fx.maker_1,
        &Side::Sell,
        &(4 * PRICE_PRECISION),
        &(10 * ONE),
    );
    // Place ask at 2.0 (lower price, must jump to front)
    let id2 = client.place_order(
        &fx.maker_1,
        &Side::Sell,
        &(2 * PRICE_PRECISION),
        &(10 * ONE),
    );
    // Place ask at 3.0 (between 2.0 and 4.0)
    let id3 = client.place_order(
        &fx.maker_2,
        &Side::Sell,
        &(3 * PRICE_PRECISION),
        &(10 * ONE),
    );

    let asks = client.get_asks();
    assert_eq!(asks.len(), 3);
    assert_eq!(asks.get(0).unwrap(), id2); // 2.0
    assert_eq!(asks.get(1).unwrap(), id3); // 3.0
    assert_eq!(asks.get(2).unwrap(), id1); // 4.0
}

#[test]
fn cancel_order_refunds_locked_quote_balance_for_buy_order() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let quote_before = StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.maker_1);
    let order_id = client.place_order(&fx.maker_1, &Side::Buy, &(2 * PRICE_PRECISION), &(50 * ONE));

    // Cancel order
    client.cancel_order(&fx.maker_1, &order_id);

    let quote_after = StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.maker_1);
    assert_eq!(quote_after, quote_before);

    // Order is deleted
    assert!(client.get_order(&order_id).is_none());
    assert_eq!(client.get_bids().len(), 0);
}

#[test]
fn cancel_order_refunds_locked_base_balance_for_sell_order() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let base_before = StellarAssetClient::new(&fx.ctx.env, &fx.base_token).balance(&fx.maker_1);
    let order_id = client.place_order(
        &fx.maker_1,
        &Side::Sell,
        &(5 * PRICE_PRECISION),
        &(75 * ONE),
    );

    client.cancel_order(&fx.maker_1, &order_id);

    let base_after = StellarAssetClient::new(&fx.ctx.env, &fx.base_token).balance(&fx.maker_1);
    assert_eq!(base_after, base_before);

    assert!(client.get_order(&order_id).is_none());
    assert_eq!(client.get_asks().len(), 0);
}

#[test]
fn cancel_order_from_unauthorized_caller_panics() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let order_id = client.place_order(&fx.maker_1, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));
    let err = client.try_cancel_order(&fx.maker_2, &order_id);
    assert!(err.is_err());
}

#[test]
fn full_market_buy_execution_clears_order_and_transfers_tokens() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    // Maker places ask: sell 50 base tokens at 2.0 quote per base
    let order_id = client.place_order(
        &fx.maker_1,
        &Side::Sell,
        &(2 * PRICE_PRECISION),
        &(50 * ONE),
    );

    let taker_base_before = StellarAssetClient::new(&fx.ctx.env, &fx.base_token).balance(&fx.taker);
    let taker_quote_before =
        StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.taker);
    let maker_quote_before =
        StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.maker_1);

    // Taker buys 50 base tokens
    let filled = client.execute_market_order(&fx.taker, &Side::Buy, &(50 * ONE));
    assert_eq!(filled, 50 * ONE);

    let taker_base_after = StellarAssetClient::new(&fx.ctx.env, &fx.base_token).balance(&fx.taker);
    let taker_quote_after =
        StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.taker);
    let maker_quote_after =
        StellarAssetClient::new(&fx.ctx.env, &fx.quote_token).balance(&fx.maker_1);

    assert_eq!(taker_base_after - taker_base_before, 50 * ONE);
    assert_eq!(taker_quote_before - taker_quote_after, 100 * ONE); // 50 * 2.0 = 100 quote
    assert_eq!(maker_quote_after - maker_quote_before, 100 * ONE);

    // Fully filled ask deleted from storage
    assert!(client.get_order(&order_id).is_none());
    assert_eq!(client.get_asks().len(), 0);
}

#[test]
fn partial_market_buy_execution_updates_order_remaining_amount() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    // Maker places ask: sell 100 base tokens at 3.0
    let order_id = client.place_order(
        &fx.maker_1,
        &Side::Sell,
        &(3 * PRICE_PRECISION),
        &(100 * ONE),
    );

    // Taker buys 40 base tokens (partial fill)
    let filled = client.execute_market_order(&fx.taker, &Side::Buy, &(40 * ONE));
    assert_eq!(filled, 40 * ONE);

    let order = client.get_order(&order_id).unwrap();
    assert_eq!(order.amount, 100 * ONE);
    assert_eq!(order.filled, 40 * ONE);

    // Order is still in asks
    let asks = client.get_asks();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks.get(0).unwrap(), order_id);

    // Now fill remaining 60 base tokens
    let filled_second = client.execute_market_order(&fx.taker, &Side::Buy, &(60 * ONE));
    assert_eq!(filled_second, 60 * ONE);

    // Now fully filled: removed from storage and asks list
    assert!(client.get_order(&order_id).is_none());
    assert_eq!(client.get_asks().len(), 0);
}

#[test]
fn market_sell_matches_bids_across_multiple_makers() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    // Maker 1 bids at 3.0 for 20 base tokens
    let id1 = client.place_order(&fx.maker_1, &Side::Buy, &(3 * PRICE_PRECISION), &(20 * ONE));
    // Maker 2 bids at 2.5 for 30 base tokens
    let id2 = client.place_order(
        &fx.maker_2,
        &Side::Buy,
        &(25 * PRICE_PRECISION / 10),
        &(30 * ONE),
    );

    // Taker sells 35 base tokens: matches all 20 at 3.0, and 15 at 2.5
    let filled = client.execute_market_order(&fx.taker, &Side::Sell, &(35 * ONE));
    assert_eq!(filled, 35 * ONE);

    // Maker 1 order fully filled and deleted
    assert!(client.get_order(&id1).is_none());

    // Maker 2 order partially filled (15/30 filled, 15 remaining)
    let order2 = client.get_order(&id2).unwrap();
    assert_eq!(order2.filled, 15 * ONE);

    let bids = client.get_bids();
    assert_eq!(bids.len(), 1);
    assert_eq!(bids.get(0).unwrap(), id2);
}
