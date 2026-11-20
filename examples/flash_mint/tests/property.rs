use proptest::prelude::*;
use soroban_sdk::{contract, contractimpl, testutils::Address as _, Address, Bytes, Env, Symbol};
use soroban_testkit_example_flash_mint::*;

#[contract]
pub struct PropBorrower;

#[contractimpl]
impl PropBorrower {
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

proptest! {
    #[test]
    fn prop_total_supply_invariant_zero_fee(amount in 1i128..1_000_000i128) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let max_loan = 1_000_000i128;
        let (token_id, client) = setup_token(&env, &admin, &treasury, 0, max_loan);

        let borrower_id = env.register(PropBorrower, ());

        // Initial background supply
        client.mint(&admin, &500_000);
        let initial_supply = client.total_supply();

        // Perform flash mint
        let success = client.flash_loan(&borrower_id, &token_id, &amount, &Bytes::new(&env));
        prop_assert!(success);

        // Core acceptance criterion: total token supply before and after is identical
        let final_supply = client.total_supply();
        prop_assert_eq!(initial_supply, final_supply);
    }

    #[test]
    fn prop_total_supply_invariant_with_fee(
        amount in 100i128..500_000i128,
        fee_bps in 1u32..500u32, // up to 5%
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let max_loan = 1_000_000i128;
        let (token_id, client) = setup_token(&env, &admin, &treasury, fee_bps, max_loan);

        let borrower_id = env.register(PropBorrower, ());

        let fee = client.flash_fee(&token_id, &amount);
        // Fund borrower with fee
        client.mint(&borrower_id, &fee);

        let supply_before_loan = client.total_supply();

        let success = client.flash_loan(&borrower_id, &token_id, &amount, &Bytes::new(&env));
        prop_assert!(success);

        // Principal burned, fee transferred to treasury => total supply unchanged
        let supply_after_loan = client.total_supply();
        prop_assert_eq!(supply_before_loan, supply_after_loan);
        prop_assert_eq!(client.balance(&treasury), fee);
        prop_assert_eq!(client.balance(&borrower_id), 0);
    }

    #[test]
    fn prop_fee_calculation_formula(
        amount in 1i128..10_000_000i128,
        fee_bps in 0u32..1_000u32,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let (token_id, client) = setup_token(&env, &admin, &treasury, fee_bps, 100_000_000);

        let expected_fee = (amount * fee_bps as i128) / 10_000i128;
        let actual_fee = client.flash_fee(&token_id, &amount);
        prop_assert_eq!(actual_fee, expected_fee);
    }

    #[test]
    fn prop_exceeding_borrow_limit_always_fails(
        excess in 1i128..100_000i128,
        max_loan in 10_000i128..500_000i128,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let (token_id, client) = setup_token(&env, &admin, &treasury, 10, max_loan);

        let borrower_id = env.register(PropBorrower, ());

        let requested = max_loan + excess;
        let res = client.try_flash_loan(&borrower_id, &token_id, &requested, &Bytes::new(&env));
        prop_assert!(res.is_err());
        prop_assert_eq!(res.unwrap_err().unwrap(), FlashMintError::ExceedsMaxFlashLoan);
    }

    #[test]
    fn prop_zero_amount_always_rejected(
        amount in -100i128..=0i128,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

        let borrower_id = env.register(PropBorrower, ());
        let res = client.try_flash_loan(&borrower_id, &token_id, &amount, &Bytes::new(&env));
        prop_assert!(res.is_err());
    }

    #[test]
    fn prop_multiple_flash_loans_preserve_supply(
        amounts in prop::collection::vec(1i128..50_000i128, 2..6),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let admin = Address::generate(&env);
        let treasury = Address::generate(&env);
        let (token_id, client) = setup_token(&env, &admin, &treasury, 0, 1_000_000);

        let borrower_id = env.register(PropBorrower, ());
        let initial_supply = client.total_supply();

        for amt in amounts {
            let ok = client.flash_loan(&borrower_id, &token_id, &amt, &Bytes::new(&env));
            prop_assert!(ok);
            prop_assert_eq!(client.total_supply(), initial_supply);
        }
    }
}
