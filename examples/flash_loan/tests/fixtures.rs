//! soroban-testkit-fixtures: building the market on a `TestContext`, several
//! borrowers from a `TestContextBuilder`, and the happy paths of single- and
//! multi-token loans with their fee attribution.

mod common;

use common::{Market, FEE_BPS, LIQUIDITY, RECEIVER_FLOAT};
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{vec, Address};
use soroban_testkit_example_flash_loan::{
    FlashLoanError, FlashLoanProvider, FlashLoanProviderClient, MAX_FEE_BPS,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

#[test]
fn a_market_built_on_a_test_context_offers_its_liquidity() {
    let ctx = TestContext::new();
    let market = Market::on(ctx.env.clone());

    assert_eq!(market.lender().max_flash_loan(&market.tokens[0]), LIQUIDITY);
    assert_eq!(market.lender().max_flash_loan(&market.tokens[1]), LIQUIDITY);
    let unlisted = Address::generate(&ctx.env);
    assert_eq!(market.lender().max_flash_loan(&unlisted), 0);
    assert_eq!(market.lender().loan_count(), 0);
}

#[test]
fn a_repaid_loan_moves_exactly_the_fee_from_receiver_to_provider() {
    let market = Market::setup();
    let amount = 1_000_000;

    let fee = market.borrow(0, amount);

    assert_eq!(fee, 900);
    assert_eq!(market.balance(0, &market.provider), LIQUIDITY + fee);
    assert_eq!(market.balance(0, &market.receiver), RECEIVER_FLOAT - fee);
    assert_eq!(market.balance(1, &market.provider), LIQUIDITY);
    assert_eq!(market.lender().loan_count(), 1);
    assert_eq!(market.borrower_contract().loans_received(), 1);
}

#[test]
fn fees_are_attributed_per_token_and_per_borrower() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let market = Market::on(ctx.env.clone());
    let (alice, bob) = (ctx.user(0), ctx.user(1));
    let lender = market.lender();
    let data = market.data();

    let a1 = lender.flash_loan(alice, &market.receiver, &market.tokens[0], &100_000, &data);
    let a2 = lender.flash_loan(alice, &market.receiver, &market.tokens[1], &200_000, &data);
    let b1 = lender.flash_loan(bob, &market.receiver, &market.tokens[0], &300_000, &data);

    assert_eq!(lender.borrower_fees(alice, &market.tokens[0]), a1);
    assert_eq!(lender.borrower_fees(alice, &market.tokens[1]), a2);
    assert_eq!(lender.borrower_fees(bob, &market.tokens[0]), b1);
    assert_eq!(lender.borrower_fees(bob, &market.tokens[1]), 0);
    assert_eq!(lender.fees_collected(&market.tokens[0]), a1 + b1);
    assert_eq!(lender.fees_collected(&market.tokens[1]), a2);
    assert_eq!(lender.loan_count(), 3);
}

#[test]
fn fees_round_up_so_no_positive_loan_is_free() {
    let market = Market::setup();
    let lender = market.lender();

    assert_eq!(lender.flash_fee(&1), 1);
    assert_eq!(lender.flash_fee(&10_000), 9);
    assert_eq!(lender.flash_fee(&10_001), 10);
    assert_eq!(lender.flash_fee(&1_000_000), 900);

    lender.set_fee_bps(&0);
    assert_eq!(lender.flash_fee(&1_000_000), 0);
    assert_eq!(market.borrow(0, 1_000_000), 0);
}

#[test]
fn a_multi_token_loan_charges_and_attributes_each_token() {
    let market = Market::setup();
    let lender = market.lender();
    let tokens = vec![
        &market.env,
        market.tokens[0].clone(),
        market.tokens[1].clone(),
    ];
    let amounts = vec![&market.env, 500_000i128, 2_000_000i128];

    let fees = lender.flash_loan_multi(
        &market.borrower,
        &market.receiver,
        &tokens,
        &amounts,
        &market.data(),
    );

    assert_eq!(fees, vec![&market.env, 450i128, 1_800i128]);
    assert_eq!(market.balance(0, &market.provider), LIQUIDITY + 450);
    assert_eq!(market.balance(1, &market.provider), LIQUIDITY + 1_800);
    assert_eq!(
        lender.borrower_fees(&market.borrower, &market.tokens[1]),
        1_800
    );
    assert_eq!(lender.loan_count(), 1);
}

#[test]
fn deposits_add_liquidity() {
    let market = Market::setup();
    let lp = Address::generate(&market.env);
    soroban_sdk::token::StellarAssetClient::new(&market.env, &market.tokens[0]).mint(&lp, &5_000);

    market.lender().deposit(&lp, &market.tokens[0], &5_000);

    assert_eq!(
        market.lender().max_flash_loan(&market.tokens[0]),
        LIQUIDITY + 5_000
    );
    assert_eq!(market.balance(0, &lp), 0);
}

#[test]
fn invalid_amounts_and_unlisted_tokens_are_refused() {
    let market = Market::setup();
    let lender = market.lender();
    let data = market.data();
    let unlisted = market
        .env
        .register_stellar_asset_contract_v2(market.admin.clone())
        .address();

    for amount in [0i128, -5] {
        assert_eq!(
            lender
                .try_flash_loan(
                    &market.borrower,
                    &market.receiver,
                    &market.tokens[0],
                    &amount,
                    &data
                )
                .unwrap_err()
                .unwrap(),
            FlashLoanError::InvalidAmount
        );
    }
    assert_eq!(
        lender
            .try_flash_loan(&market.borrower, &market.receiver, &unlisted, &1, &data)
            .unwrap_err()
            .unwrap(),
        FlashLoanError::UnsupportedToken
    );
    assert_eq!(
        lender
            .try_flash_loan(
                &market.borrower,
                &market.receiver,
                &market.tokens[0],
                &(LIQUIDITY + 1),
                &data
            )
            .unwrap_err()
            .unwrap(),
        FlashLoanError::InsufficientLiquidity
    );
    assert_eq!(
        lender.try_flash_fee(&0).unwrap_err().unwrap(),
        FlashLoanError::InvalidAmount
    );
}

#[test]
fn malformed_multi_token_requests_are_refused() {
    let market = Market::setup();
    let lender = market.lender();
    let data = market.data();
    let env = &market.env;

    let mismatched = lender.try_flash_loan_multi(
        &market.borrower,
        &market.receiver,
        &vec![env, market.tokens[0].clone()],
        &vec![env, 1i128, 2i128],
        &data,
    );
    assert_eq!(
        mismatched.unwrap_err().unwrap(),
        FlashLoanError::LengthMismatch
    );

    let empty = lender.try_flash_loan_multi(
        &market.borrower,
        &market.receiver,
        &soroban_sdk::Vec::new(env),
        &soroban_sdk::Vec::new(env),
        &data,
    );
    assert_eq!(empty.unwrap_err().unwrap(), FlashLoanError::LengthMismatch);

    let duplicate = lender.try_flash_loan_multi(
        &market.borrower,
        &market.receiver,
        &vec![env, market.tokens[0].clone(), market.tokens[0].clone()],
        &vec![env, 1i128, 1i128],
        &data,
    );
    assert_eq!(
        duplicate.unwrap_err().unwrap(),
        FlashLoanError::DuplicateToken
    );
}

#[test]
fn configuration_is_admin_controlled_and_bounded() {
    let ctx = TestContext::new();
    let market = Market::on(ctx.env.clone());
    let lender = market.lender();

    assert_eq!(
        lender
            .try_initialize(&market.admin, &FEE_BPS)
            .unwrap_err()
            .unwrap(),
        FlashLoanError::AlreadyInitialized
    );
    assert_eq!(
        lender
            .try_set_fee_bps(&(MAX_FEE_BPS + 1))
            .unwrap_err()
            .unwrap(),
        FlashLoanError::InvalidFee
    );

    let fresh = ctx.env.register(FlashLoanProvider, ());
    let fresh = FlashLoanProviderClient::new(&ctx.env, &fresh);
    assert_eq!(
        fresh
            .try_initialize(&ctx.admin, &(MAX_FEE_BPS + 1))
            .unwrap_err()
            .unwrap(),
        FlashLoanError::InvalidFee
    );
    assert_eq!(
        fresh.try_flash_fee(&100).unwrap_err().unwrap(),
        FlashLoanError::NotInitialized
    );
}
