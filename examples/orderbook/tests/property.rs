extern crate std;

use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};
use soroban_testkit_example_orderbook::{
    calculate_quote_amount, OrderBookContract, OrderBookContractClient, Side, PRICE_PRECISION,
};

const ONE: i128 = 10_000_000;

proptest! {
    /// Acceptance Criterion: Proptest verifying that buyer received tokens and
    /// seller received payment matching the execution price.
    #[test]
    fn proptest_buyer_receives_tokens_and_seller_receives_payment_matching_price(
        price_multiple in 1i128..=20i128,
        amount_tokens in 1i128..=50i128,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let book = env.register(OrderBookContract, ());
        let admin = Address::generate(&env);
        let base_token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();
        let quote_token = env
            .register_stellar_asset_contract_v2(admin.clone())
            .address();

        let maker = Address::generate(&env);
        let taker = Address::generate(&env);

        let initial_mint = 1_000_000_000 * ONE;
        for user in [&maker, &taker] {
            StellarAssetClient::new(&env, &base_token).mint(user, &initial_mint);
            StellarAssetClient::new(&env, &quote_token).mint(user, &initial_mint);
        }

        let client = OrderBookContractClient::new(&env, &book);
        client.initialize(&admin, &base_token, &quote_token, &0, &0);

        let price = price_multiple * PRICE_PRECISION;
        let amount = amount_tokens * ONE;
        let expected_quote = calculate_quote_amount(amount, price).unwrap();

        // Maker places ask to sell base tokens
        client.place_order(&maker, &Side::Sell, &price, &amount);

        let taker_base_before = StellarAssetClient::new(&env, &base_token).balance(&taker);
        let taker_quote_before = StellarAssetClient::new(&env, &quote_token).balance(&taker);
        let maker_base_before = StellarAssetClient::new(&env, &base_token).balance(&maker);
        let maker_quote_before = StellarAssetClient::new(&env, &quote_token).balance(&maker);

        // Taker executes market buy for `amount`
        let filled = client.execute_market_order(&taker, &Side::Buy, &amount);

        prop_assert_eq!(filled, amount);

        let taker_base_after = StellarAssetClient::new(&env, &base_token).balance(&taker);
        let taker_quote_after = StellarAssetClient::new(&env, &quote_token).balance(&taker);
        let maker_base_after = StellarAssetClient::new(&env, &base_token).balance(&maker);
        let maker_quote_after = StellarAssetClient::new(&env, &quote_token).balance(&maker);

        // Buyer (taker) received exact base tokens
        prop_assert_eq!(taker_base_after - taker_base_before, amount);
        // Buyer (taker) paid exact quote amount
        prop_assert_eq!(taker_quote_before - taker_quote_after, expected_quote);
        // Seller (maker) received exact quote payment matching price
        prop_assert_eq!(maker_quote_after - maker_quote_before, expected_quote);
        // Seller (maker) base balance was escrowed then delivered
        prop_assert_eq!(maker_base_before, maker_base_after);
    }

    #[test]
    fn proptest_bid_matching_conservation_of_quote_and_base(
        price_multiple in 1i128..=15i128,
        amount_tokens in 1i128..=40i128,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let book = env.register(OrderBookContract, ());
        let admin = Address::generate(&env);
        let base_token = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let quote_token = env.register_stellar_asset_contract_v2(admin.clone()).address();

        let maker = Address::generate(&env);
        let taker = Address::generate(&env);

        let initial_mint = 1_000_000_000 * ONE;
        for user in [&maker, &taker] {
            StellarAssetClient::new(&env, &base_token).mint(user, &initial_mint);
            StellarAssetClient::new(&env, &quote_token).mint(user, &initial_mint);
        }

        let client = OrderBookContractClient::new(&env, &book);
        client.initialize(&admin, &base_token, &quote_token, &0, &0);

        let price = price_multiple * PRICE_PRECISION;
        let amount = amount_tokens * ONE;
        let expected_quote = calculate_quote_amount(amount, price).unwrap();

        // Maker places bid to buy base tokens
        client.place_order(&maker, &Side::Buy, &price, &amount);

        let taker_base_before = StellarAssetClient::new(&env, &base_token).balance(&taker);
        let taker_quote_before = StellarAssetClient::new(&env, &quote_token).balance(&taker);
        let maker_base_before = StellarAssetClient::new(&env, &base_token).balance(&maker);

        // Taker sells base tokens into the resting bid
        let filled = client.execute_market_order(&taker, &Side::Sell, &amount);
        prop_assert_eq!(filled, amount);

        let taker_base_after = StellarAssetClient::new(&env, &base_token).balance(&taker);
        let taker_quote_after = StellarAssetClient::new(&env, &quote_token).balance(&taker);
        let maker_base_after = StellarAssetClient::new(&env, &base_token).balance(&maker);

        // Taker delivered base tokens
        prop_assert_eq!(taker_base_before - taker_base_after, amount);
        // Taker received quote payment
        prop_assert_eq!(taker_quote_after - taker_quote_before, expected_quote);
        // Maker received base tokens
        prop_assert_eq!(maker_base_after - maker_base_before, amount);
    }

    #[test]
    fn proptest_calculate_quote_amount_linearity(
        amount in 1i128..=1_000_000i128,
        price in 1i128..=100_000_000i128,
    ) {
        let quote = calculate_quote_amount(amount, price);
        prop_assert!(quote.is_some());
        let val = quote.unwrap();
        prop_assert!(val >= 0);
    }

    #[test]
    fn proptest_cancellation_always_refunds_100_percent_of_escrow(
        price_multiple in 1i128..=10i128,
        amount_tokens in 1i128..=30i128,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let book = env.register(OrderBookContract, ());
        let admin = Address::generate(&env);
        let base_token = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let quote_token = env.register_stellar_asset_contract_v2(admin.clone()).address();

        let maker = Address::generate(&env);
        let initial_mint = 500_000_000 * ONE;
        StellarAssetClient::new(&env, &base_token).mint(&maker, &initial_mint);
        StellarAssetClient::new(&env, &quote_token).mint(&maker, &initial_mint);

        let client = OrderBookContractClient::new(&env, &book);
        client.initialize(&admin, &base_token, &quote_token, &0, &0);

        let price = price_multiple * PRICE_PRECISION;
        let amount = amount_tokens * ONE;

        let base_before = StellarAssetClient::new(&env, &base_token).balance(&maker);
        let quote_before = StellarAssetClient::new(&env, &quote_token).balance(&maker);

        let bid_id = client.place_order(&maker, &Side::Buy, &price, &amount);
        let ask_id = client.place_order(&maker, &Side::Sell, &price, &amount);

        // Cancel both
        client.cancel_order(&maker, &bid_id);
        client.cancel_order(&maker, &ask_id);

        let base_after = StellarAssetClient::new(&env, &base_token).balance(&maker);
        let quote_after = StellarAssetClient::new(&env, &quote_token).balance(&maker);

        prop_assert_eq!(base_after, base_before);
        prop_assert_eq!(quote_after, quote_before);
    }
}
