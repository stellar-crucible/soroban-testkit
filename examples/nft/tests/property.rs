extern crate std;

use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, String};
use soroban_testkit_example_nft::{NftContract, NftContractClient, BPS_DENOMINATOR};

proptest! {
    /// Acceptance Criterion: Proptest suite proving token uniqueness and
    /// single-owner invariant across arbitrary token IDs and transfers.
    #[test]
    fn proptest_token_uniqueness_and_single_owner_invariant(
        token_id in 1u32..=1000u32,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let nft = env.register(NftContract, ());
        let admin = Address::generate(&env);
        let creator = Address::generate(&env);
        let user_a = Address::generate(&env);
        let user_b = Address::generate(&env);

        let client = NftContractClient::new(&env, &nft);
        client.initialize(
            &admin,
            &String::from_str(&env, "Test"),
            &String::from_str(&env, "TST"),
            &creator,
            &500,
        );

        // Mint token to user_a
        let uri = String::from_str(&env, "ipfs://nft");
        client.mint(&user_a, &token_id, &uri);

        // Single-owner invariant: only user_a owns token_id
        prop_assert_eq!(client.owner_of(&token_id), Some(user_a.clone()));
        prop_assert_eq!(client.balance_of(&user_a), 1);
        prop_assert_eq!(client.balance_of(&user_b), 0);

        // Uniqueness invariant: cannot mint duplicate token_id
        let duplicate_attempt = client.try_mint(&user_b, &token_id, &uri);
        prop_assert!(duplicate_attempt.is_err());

        // Transfer to user_b
        client.transfer(&user_a, &user_b, &token_id);

        // Single-owner invariant preserved: now only user_b owns token_id
        prop_assert_eq!(client.owner_of(&token_id), Some(user_b.clone()));
        prop_assert_eq!(client.balance_of(&user_a), 0);
        prop_assert_eq!(client.balance_of(&user_b), 1);
        prop_assert_eq!(client.total_supply(), 1);
    }

    #[test]
    fn proptest_royalty_calculation_strictly_bounded(
        sale_price in 1i128..=10_000_000_000_000i128,
        royalty_bps in 0u32..=10_000u32,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let nft = env.register(NftContract, ());
        let admin = Address::generate(&env);
        let creator = Address::generate(&env);

        let client = NftContractClient::new(&env, &nft);
        client.initialize(
            &admin,
            &String::from_str(&env, "RoyaltyTest"),
            &String::from_str(&env, "ROY"),
            &creator,
            &royalty_bps,
        );

        let (recip, royalty_amount) = client.calculate_royalty(&sale_price);

        prop_assert_eq!(recip, creator);
        prop_assert!(royalty_amount >= 0);
        prop_assert!(royalty_amount <= sale_price);

        let expected = (sale_price * royalty_bps as i128) / BPS_DENOMINATOR;
        prop_assert_eq!(royalty_amount, expected);
    }

    #[test]
    fn proptest_batch_mint_balance_conservation(
        batch_size in 1usize..=20usize,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let nft = env.register(NftContract, ());
        let admin = Address::generate(&env);
        let recipient = Address::generate(&env);

        let client = NftContractClient::new(&env, &nft);
        client.initialize(
            &admin,
            &String::from_str(&env, "Batch"),
            &String::from_str(&env, "BAT"),
            &admin,
            &250,
        );

        let mut token_ids = soroban_sdk::Vec::new(&env);
        let mut uris = soroban_sdk::Vec::new(&env);

        for i in 1..=(batch_size as u32) {
            token_ids.push_back(i);
            uris.push_back(String::from_str(&env, "uri"));
        }

        let minted_count = client.batch_mint(&recipient, &token_ids, &uris);
        prop_assert_eq!(minted_count as usize, batch_size);
        prop_assert_eq!(client.balance_of(&recipient) as usize, batch_size);
        prop_assert_eq!(client.total_supply() as usize, batch_size);
    }
}
