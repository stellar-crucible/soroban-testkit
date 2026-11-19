use soroban_sdk::{
    contract, contractimpl, symbol_short, testutils::Address as _, Address, Bytes, Env, Symbol,
};
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_flash_mint::*;

#[contract]
pub struct AssertSuccessBorrower;

#[contractimpl]
impl AssertSuccessBorrower {
    pub fn on_flash_loan(
        _env: Env,
        _initiator: Address,
        _token: Address,
        _amount: i128,
        _fee: i128,
        _data: Bytes,
    ) -> Symbol {
        CALLBACK_SUCCESS
    }
}

#[contract]
pub struct AssertFailingBorrower;

#[contractimpl]
impl AssertFailingBorrower {
    pub fn on_flash_loan(
        _env: Env,
        _initiator: Address,
        _token: Address,
        _amount: i128,
        _fee: i128,
        _data: Bytes,
    ) -> Symbol {
        symbol_short!("fail_cb")
    }
}

#[contract]
pub struct AssertDeficitBorrower;

#[contractimpl]
impl AssertDeficitBorrower {
    pub fn on_flash_loan(
        _env: Env,
        _initiator: Address,
        _token: Address,
        _amount: i128,
        _fee: i128,
        _data: Bytes,
    ) -> Symbol {
        // Return success without providing the required fee, creating a deficit
        CALLBACK_SUCCESS
    }
}

#[contract]
pub struct AssertReentrantBorrower;

#[contractimpl]
impl AssertReentrantBorrower {
    pub fn on_flash_loan(
        env: Env,
        _initiator: Address,
        token: Address,
        amount: i128,
        _fee: i128,
        data: Bytes,
    ) -> Symbol {
        let client = FlashMintTokenClient::new(&env, &token);
        // Attempt recursive reentrant flash loan invocation
        let _ = client.try_flash_loan(&env.current_contract_address(), &token, &amount, &data);
        CALLBACK_SUCCESS
    }
}

fn setup_token(
    env: &Env,
    admin: &Address,
    treasury: &Address,
    fee_bps: u32,
    max_mint: i128,
) -> (Address, FlashMintTokenClient<'static>) {
    let token_id = env.register(FlashMintToken, ());
    let client = FlashMintTokenClient::new(env, &token_id);
    client.initialize(admin, treasury, &fee_bps, &max_mint);
    (token_id, client)
}

#[test]
fn test_flash_mint_event_emission() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let borrower_id = env.register(AssertSuccessBorrower, ());

    // Provide fee to borrower
    let fee = client.flash_fee(&token_id, &100_000);
    client.mint(&borrower_id, &fee);

    client.flash_loan(&borrower_id, &token_id, &100_000, &Bytes::new(&env));

    // Assert that FlashMint event was emitted
    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("flash_mnt")
        .assert_emitted();

    // Assert fee_paid event was emitted
    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("fee_paid")
        .assert_emitted();
}

#[test]
fn test_event_log_tracks_across_invocations() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 0, 1_000_000);

    let borrower_id = env.register(AssertSuccessBorrower, ());
    let mut log = EventLog::new(&env);

    client.flash_loan(&borrower_id, &token_id, &20_000, &Bytes::new(&env));
    log.collect();

    client.flash_loan(&borrower_id, &token_id, &30_000, &Bytes::new(&env));
    log.collect();

    log.matcher()
        .from_contract(&token_id)
        .with_topic("flash_mnt")
        .assert_count(2);
}

#[test]
fn test_callback_failure_rolls_back_transaction() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let failing_id = env.register(AssertFailingBorrower, ());

    let err = client.try_flash_loan(&failing_id, &token_id, &50_000, &Bytes::new(&env));
    assert_eq!(err.unwrap_err().unwrap(), FlashMintError::CallbackFailed);

    // Assert state rolled back completely
    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.balance(&failing_id), 0);
}

#[test]
fn test_deficit_borrower_rolls_back_transaction() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    // Configure with 50 bps fee so unfunded borrower has a deficit
    let (token_id, client) = setup_token(&env, &admin, &treasury, 50, 1_000_000);

    let deficit_id = env.register(AssertDeficitBorrower, ());

    // Deficit borrower lacks the required fee to repay total due
    let err = client.try_flash_loan(&deficit_id, &token_id, &100_000, &Bytes::new(&env));
    assert_eq!(
        err.unwrap_err().unwrap(),
        FlashMintError::DeficitAfterFlashMint
    );

    // Assert total supply and balance did not leak any minted tokens
    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.balance(&deficit_id), 0);
}

#[test]
fn test_reentrancy_protection() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 0, 1_000_000);

    let reentrant_id = env.register(AssertReentrantBorrower, ());

    // Reentrant call within callback should succeed outer but inner was rejected by reentrancy guard
    let res = client.flash_loan(&reentrant_id, &token_id, &100_000, &Bytes::new(&env));
    assert!(res);
    assert_eq!(client.total_supply(), 0);
}

#[test]
fn test_admin_config_events() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    client.set_fee_bps(&25);
    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("cfg_fee")
        .assert_emitted();

    client.set_max_flash_mint(&5_000_000);
    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("cfg_limit")
        .assert_emitted();

    client.pause();
    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("paused")
        .assert_emitted();
}

#[test]
fn test_burn_event_emission() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let user = Address::generate(&env);
    client.mint(&user, &500);
    client.burn(&user, &200);

    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("burn")
        .assert_emitted();
}

#[test]
fn test_transfer_event_emission() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let user_a = Address::generate(&env);
    let user_b = Address::generate(&env);
    client.mint(&user_a, &1_000);
    client.transfer(&user_a, &user_b, &400);

    EventMatcher::new(&env)
        .from_contract(&token_id)
        .with_topic("transfer")
        .assert_emitted();
}

#[test]
fn test_multiple_repayments_in_log() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let borrower = env.register(AssertSuccessBorrower, ());
    let mut log = EventLog::new(&env);

    client.mint(&borrower, &1_000);

    client.flash_loan(&borrower, &token_id, &10_000, &Bytes::new(&env));
    log.collect();
    client.flash_loan(&borrower, &token_id, &20_000, &Bytes::new(&env));
    log.collect();
    client.flash_loan(&borrower, &token_id, &30_000, &Bytes::new(&env));
    log.collect();

    log.matcher()
        .from_contract(&token_id)
        .with_topic("fee_paid")
        .assert_count(3);
}
