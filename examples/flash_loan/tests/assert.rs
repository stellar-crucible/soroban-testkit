//! soroban-testkit-assert: exact errors and full rollback when a receiver
//! does not repay, the events a loan emits across three contracts, and the
//! authorization a loan needs (one borrower signature covers the whole tree).

mod common;

use common::{Market, LIQUIDITY, RECEIVER_FLOAT};
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::{vec, Address, Error, IntoVal};
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_assert::{
    assert_auth_matches, assert_contract_error, assert_event_topic_count, assert_host_error,
};
use soroban_testkit_example_flash_loan::{FlashLoanError, ReceiverError, RepayMode};

/// Nothing a failed loan touched may survive it.
fn assert_rolled_back(market: &Market) {
    for index in 0..2 {
        assert_eq!(market.balance(index, &market.provider), LIQUIDITY);
        assert_eq!(market.balance(index, &market.receiver), RECEIVER_FLOAT);
        assert_eq!(market.lender().fees_collected(&market.tokens[index]), 0);
    }
    assert_eq!(market.lender().loan_count(), 0);
    assert_eq!(market.borrower_contract().loans_received(), 0);
}

fn try_borrow(
    market: &Market,
    amount: i128,
) -> Result<Result<i128, soroban_sdk::Error>, Result<FlashLoanError, soroban_sdk::InvokeError>> {
    market.lender().try_flash_loan(
        &market.borrower,
        &market.receiver,
        &market.tokens[0],
        &amount,
        &market.data(),
    )
}

// --- rollback ---------------------------------------------------------------

#[test]
fn repaying_one_stroop_short_rolls_the_whole_loan_back() {
    let market = Market::setup();
    market.set_mode(RepayMode::ShortBy(1));

    assert_contract_error!(
        try_borrow(&market, 1_000_000),
        FlashLoanError::RepaymentShort
    );
    assert_rolled_back(&market);
}

#[test]
fn returning_only_the_principal_is_a_short_repayment() {
    let market = Market::setup();
    market.set_mode(RepayMode::PrincipalOnly);

    assert_contract_error!(
        try_borrow(&market, 5_000_000),
        FlashLoanError::RepaymentShort
    );
    assert_rolled_back(&market);
}

#[test]
fn a_receiver_that_refuses_the_callback_undoes_the_transfer() {
    let market = Market::setup();
    market.set_mode(RepayMode::Reject);

    assert_contract_error!(try_borrow(&market, 1_000), FlashLoanError::CallbackRejected);
    assert_rolled_back(&market);
}

#[test]
fn one_short_token_rolls_back_every_leg_of_a_multi_token_loan() {
    let market = Market::setup();
    market.set_mode(RepayMode::ShortBy(1));
    let env = &market.env;

    assert_contract_error!(
        market.lender().try_flash_loan_multi(
            &market.borrower,
            &market.receiver,
            &vec![env, market.tokens[0].clone(), market.tokens[1].clone()],
            &vec![env, 10_000i128, 20_000i128],
            &market.data(),
        ),
        FlashLoanError::RepaymentShort
    );
    assert_rolled_back(&market);
}

#[test]
fn a_receiver_error_surfaces_with_the_receivers_code() {
    let market = Market::setup();
    let stranger = Address::generate(&market.env);
    market
        .borrower_contract()
        .configure(&stranger, &RepayMode::Full);

    let outcome = try_borrow(&market, 1_000);

    assert_host_error!(
        outcome,
        Error::from_contract_error(ReceiverError::UnknownLender as u32)
    );
    assert_rolled_back(&market);
}

// --- events -------------------------------------------------------------------

#[test]
fn a_loan_emits_one_provider_event_and_two_token_transfers() {
    let market = Market::setup();

    market.borrow(0, 1_000_000);

    EventMatcher::new(&market.env)
        .from_contract(&market.provider)
        .with_topic("flash")
        .assert_emitted();
    assert_event_topic_count!(market.env, from = market.tokens[0], "transfer", 2);
    assert_event_topic_count!(market.env, from = market.tokens[1], "transfer", 0);
}

#[test]
fn a_multi_token_loan_emits_transfers_on_both_tokens() {
    let market = Market::setup();
    let env = &market.env;

    market.lender().flash_loan_multi(
        &market.borrower,
        &market.receiver,
        &vec![env, market.tokens[0].clone(), market.tokens[1].clone()],
        &vec![env, 1_000i128, 2_000i128],
        &market.data(),
    );

    EventMatcher::new(env)
        .from_contract(&market.provider)
        .with_topic("flash_mul")
        .assert_emitted();
    assert_event_topic_count!(market.env, from = market.tokens[0], "transfer", 2);
    assert_event_topic_count!(market.env, from = market.tokens[1], "transfer", 2);
}

// --- authorization ------------------------------------------------------------

#[test]
fn a_loan_is_authorized_by_the_borrower() {
    let market = Market::setup();

    market.borrow(0, 1_000);

    assert_auth_matches!(market.env, {
        invoker: market.borrower,
        function: "flash_loan",
    });
}

/// The borrower signs only the root call. The principal transfer is made by
/// the provider from its own balance, the callback authenticates the provider
/// as its direct caller, and the repayment is made by the receiver from its
/// own balance, so no nested authorization entry is needed.
#[test]
fn one_borrower_signature_covers_the_whole_sub_invocation_tree() {
    let market = Market::setup();
    let env = &market.env;
    let amount = 50_000i128;
    let data = market.data();

    let fee = market
        .lender()
        .mock_auths(&[MockAuth {
            address: &market.borrower,
            invoke: &MockAuthInvoke {
                contract: &market.provider,
                fn_name: "flash_loan",
                args: (
                    market.borrower.clone(),
                    market.receiver.clone(),
                    market.tokens[0].clone(),
                    amount,
                    data.clone(),
                )
                    .into_val(env),
                sub_invokes: &[],
            },
        }])
        .flash_loan(
            &market.borrower,
            &market.receiver,
            &market.tokens[0],
            &amount,
            &data,
        );

    assert_eq!(fee, 45);
}

#[test]
fn a_loan_without_the_borrowers_signature_is_refused() {
    let market = Market::setup();

    let outcome = market.lender().mock_auths(&[]).try_flash_loan(
        &market.borrower,
        &market.receiver,
        &market.tokens[0],
        &1_000,
        &market.data(),
    );

    assert!(outcome.is_err(), "an unsigned loan must not succeed");
    assert_rolled_back(&market);
}
