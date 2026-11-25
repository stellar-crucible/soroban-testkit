extern crate std;

use proptest::prelude::*;
use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env, Symbol};
use soroban_testkit_example_escrow::{
    EscrowContract, EscrowContractClient, EscrowError, EscrowStatus,
};

proptest! {
    #[test]
    fn prop_unauthorized_parties_cannot_claim_or_release(
        total in 1_000i128..=100_000,
        milestone_amt in 100i128..=500,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let arbiter = Address::generate(&env);
        let attacker = Address::generate(&env);

        let token = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        StellarAssetClient::new(&env, &token).mint(&buyer, &(total * 2));

        let contract_id = env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&env, &contract_id);

        client.initialize(&buyer, &seller, &arbiter, &token, &total, &500);
        client.add_milestone(&buyer, &1, &Symbol::new(&env, "m1"), &milestone_amt);
        client.deposit(&buyer, &total);

        // Attacker attempts to approve milestone
        let res_approve = client.try_approve_milestone(&attacker, &1);
        prop_assert_eq!(res_approve, Err(Ok(EscrowError::Unauthorized)));

        // Attacker attempts to resolve dispute
        let res_resolve = client.try_resolve_dispute(&attacker, &(total / 2), &(total / 2));
        prop_assert_eq!(res_resolve, Err(Ok(EscrowError::Unauthorized)));

        // Attacker attempts to claim refund
        let res_refund = client.try_claim_refund(&attacker);
        prop_assert_eq!(res_refund, Err(Ok(EscrowError::Unauthorized)));
    }

    #[test]
    fn prop_dispute_resolution_conserves_payouts(
        total in 10_000i128..=100_000,
        split_ratio in 1i128..=99,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let arbiter = Address::generate(&env);

        let token = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        StellarAssetClient::new(&env, &token).mint(&buyer, &(total * 2));

        let contract_id = env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&env, &contract_id);

        client.initialize(&buyer, &seller, &arbiter, &token, &total, &500);
        client.deposit(&buyer, &total);
        client.raise_dispute(&buyer, &Symbol::new(&env, "dispute"));

        let buyer_payout = (total * split_ratio) / 100;
        let seller_payout = total - buyer_payout;

        prop_assert_eq!(buyer_payout + seller_payout, total);

        let res = client.try_resolve_dispute(&arbiter, &buyer_payout, &seller_payout);
        prop_assert!(res.is_ok());
        prop_assert_eq!(client.get_status(), EscrowStatus::Resolved);
        prop_assert_eq!(client.get_remaining_funds(), 0);
    }

    #[test]
    fn prop_deposits_cannot_exceed_total_allocation(
        total in 1_000i128..=50_000,
        excess in 1i128..=5_000,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let arbiter = Address::generate(&env);

        let token = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        StellarAssetClient::new(&env, &token).mint(&buyer, &(total * 3));

        let contract_id = env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&env, &contract_id);

        client.initialize(&buyer, &seller, &arbiter, &token, &total, &500);

        let res = client.try_deposit(&buyer, &(total + excess));
        prop_assert_eq!(res, Err(Ok(EscrowError::ExceedsTotal)));
    }
}
