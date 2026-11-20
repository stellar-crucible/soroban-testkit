use soroban_sdk::{contract, contractimpl, testutils::Address as _, Address, Bytes, Env, Symbol};
use soroban_testkit_example_flash_mint::*;
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::{CongestionSimulator, TestContext};

#[contract]
pub struct FixtureSuccessBorrower;

#[contractimpl]
impl FixtureSuccessBorrower {
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
fn test_fixture_context_setup_and_init() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 1_000_000);

    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.max_flash_loan(&token_id), 1_000_000);
}

#[test]
fn test_borrow_limits_and_configuration() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let treasury = ctx.user(0);
    let (_token_id, client) = setup_token(&ctx.env, &ctx.admin, treasury, 15, 500_000);

    // Initial limit
    assert_eq!(client.max_flash_loan(&client.address), 500_000);

    // Admin raises limit
    client.set_max_flash_mint(&2_000_000);
    assert_eq!(client.max_flash_loan(&client.address), 2_000_000);

    // Admin lowers limit
    client.set_max_flash_mint(&100_000);
    assert_eq!(client.max_flash_loan(&client.address), 100_000);
}

#[test]
fn test_fee_computation_across_amounts() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (_token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 50, 10_000_000); // 50 bps = 0.5%

    // 10,000 * 50 / 10,000 = 50
    assert_eq!(client.flash_fee(&client.address, &10_000), 50);

    // 1,000,000 * 50 / 10,000 = 5,000
    assert_eq!(client.flash_fee(&client.address, &1_000_000), 5_000);

    // Admin updates fee to 100 bps (1%)
    client.set_fee_bps(&100);
    assert_eq!(client.flash_fee(&client.address, &10_000), 100);

    // Zero fee setting (0 bps)
    client.set_fee_bps(&0);
    assert_eq!(client.flash_fee(&client.address, &100_000), 0);
}

#[test]
fn test_zero_amount_borrow_rejected() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (_token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 1_000_000);

    let err = client.try_flash_fee(&client.address, &0);
    assert_eq!(err.unwrap_err().unwrap(), FlashMintError::ZeroAmount);
}

#[test]
fn test_exceeding_max_borrow_rejected() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 100_000);

    let borrower_id = ctx.env.register(FixtureSuccessBorrower, ());
    let err = client.try_flash_loan(&borrower_id, &token_id, &100_001, &Bytes::new(&ctx.env));

    assert_eq!(
        err.unwrap_err().unwrap(),
        FlashMintError::ExceedsMaxFlashLoan
    );
}

#[test]
fn test_invalid_token_address_rejected() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (_token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 1_000_000);

    let fake_token = Address::generate(&ctx.env);
    let borrower_id = ctx.env.register(FixtureSuccessBorrower, ());

    let err = client.try_flash_loan(&borrower_id, &fake_token, &10_000, &Bytes::new(&ctx.env));
    assert_eq!(err.unwrap_err().unwrap(), FlashMintError::InvalidToken);
}

#[test]
fn test_pause_and_unpause_behavior() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 500_000);

    let borrower_id = ctx.env.register(FixtureSuccessBorrower, ());

    // Pause
    client.pause();
    assert_eq!(client.max_flash_loan(&token_id), 0);

    let err = client.try_flash_loan(&borrower_id, &token_id, &50_000, &Bytes::new(&ctx.env));
    assert_eq!(err.unwrap_err().unwrap(), FlashMintError::Paused);

    // Unpause
    client.unpause();
    assert_eq!(client.max_flash_loan(&token_id), 500_000);
}

#[test]
fn test_successful_flash_mint_with_zero_fee() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 0, 1_000_000);

    let borrower_id = ctx.env.register(FixtureSuccessBorrower, ());

    let res = client.flash_loan(&borrower_id, &token_id, &250_000, &Bytes::new(&ctx.env));
    assert!(res);
    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.balance(&borrower_id), 0);
}

#[test]
fn test_congestion_simulation_across_flash_loans() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 0, 1_000_000);

    let borrower_id = ctx.env.register(FixtureSuccessBorrower, ());
    let mut sim = CongestionSimulator::new()
        .with_delay_range(1, 3)
        .with_seconds_per_ledger(5);

    for _ in 0..3 {
        let (rep, success) = sim.execute_with_delay(&ctx.env, || {
            client.flash_loan(&borrower_id, &token_id, &50_000, &Bytes::new(&ctx.env))
        });
        assert!(success);
        assert!(rep.ledgers_delayed >= 1);
    }
}

#[test]
fn test_admin_cannot_initialize_twice() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (_token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 1_000_000);

    let err = client.try_initialize(&ctx.admin, &treasury, &20, &500_000);
    assert_eq!(
        err.unwrap_err().unwrap(),
        FlashMintError::AlreadyInitialized
    );
}

#[test]
fn test_uninitialized_contract_rejects_mint() {
    let ctx = TestContext::new();
    let token_id = ctx.env.register(FlashMintToken, ());
    let client = FlashMintTokenClient::new(&ctx.env, &token_id);

    let user = Address::generate(&ctx.env);
    let err = client.try_mint(&user, &1_000);
    assert_eq!(err.unwrap_err().unwrap(), FlashMintError::NotInitialized);
}

#[test]
fn test_transfer_insufficient_balance_rejected() {
    let ctx = TestContext::new();
    let treasury = Address::generate(&ctx.env);
    let (_token_id, client) = setup_token(&ctx.env, &ctx.admin, &treasury, 10, 1_000_000);

    let user_a = Address::generate(&ctx.env);
    let user_b = Address::generate(&ctx.env);

    client.mint(&user_a, &100);
    let err = client.try_transfer(&user_a, &user_b, &150);
    assert_eq!(
        err.unwrap_err().unwrap(),
        FlashMintError::InsufficientBalance
    );
}
