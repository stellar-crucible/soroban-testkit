//! One crate per file: this suite drives `examples/vesting_escrow` through
//! `soroban-testkit-generators` and `proptest`, verifying vesting invariants.

use proptest::prelude::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};
use soroban_testkit_example_vesting_escrow::{VestingEscrow, VestingEscrowClient};
use soroban_testkit_generators::strategies::timestamp;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn claimed_plus_claimable_plus_unvested_plus_returned_equals_total(
        total in 1_000i128..=1_000_000i128,
        cliff in 0u64..=50u64,
        duration in 51u64..=200u64,
        start in timestamp(),
        revoke_at in 0u64..=300u64,
        revocable in proptest::bool::ANY,
    ) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(start);

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register(VestingEscrow, ());
        let client = VestingEscrowClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin).address();
        StellarAssetClient::new(&env, &token_id).mint(&admin, &total);

        let id = client.create_schedule(&admin, &beneficiary, &token_id, &total, &start, &cliff, &duration, &revocable);

        // Advance, optionally claim, optionally revoke, and check the invariant.
        let t1 = start.saturating_add(30);
        env.ledger().set_timestamp(t1);
        let claimable = client.claimable(&id);
        if claimable > 0 {
            client.claim(&id);
        }

        let t2 = t1.saturating_add(revoke_at);
        env.ledger().set_timestamp(t2);
        if revocable && client.unvested(&id) > 0 {
            let _ = client.try_revoke(&id);
        }

        let claimed = client.claimed(&id);
        let claimable = client.claimable(&id);
        let unvested = client.unvested(&id);
        let returned = client.returned(&id);

        prop_assert_eq!(claimed + claimable + unvested + returned, total);

        // Token conservation: contract balance + beneficiary + admin returned == total.
        let token = soroban_sdk::token::Client::new(&env, &token_id);
        prop_assert_eq!(
            token.balance(&contract_id) + token.balance(&beneficiary) + token.balance(&admin),
            total
        );
    }

    #[test]
    fn vesting_is_monotonic_in_time(
        total in 1_000i128..=1_000_000i128,
        cliff in 0u64..=40u64,
        duration in 50u64..=150u64,
        start in timestamp(),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(start);

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register(VestingEscrow, ());
        let client = VestingEscrowClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin).address();
        StellarAssetClient::new(&env, &token_id).mint(&admin, &total);

        client.create_schedule(&admin, &beneficiary, &token_id, &total, &start, &cliff, &duration, &true);

        let mut prev = 0i128;
        for elapsed in [0u64, 10, 25, 50, 75, 100, 200] {
            env.ledger().set_timestamp(start.saturating_add(elapsed));
            let v = client.vested(&0);
            prop_assert!(v >= prev);
            prop_assert!(v <= total);
            prev = v;
        }
        prop_assert_eq!(client.vested(&0), total);
    }

    #[test]
    fn claims_never_exceed_vested(
        total in 1_000i128..=100_000i128,
        start in timestamp(),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(start);

        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register(VestingEscrow, ());
        let client = VestingEscrowClient::new(&env, &contract_id);

        let token_admin = Address::generate(&env);
        let token_id = env.register_stellar_asset_contract_v2(token_admin).address();
        StellarAssetClient::new(&env, &token_id).mint(&admin, &total);

        client.create_schedule(&admin, &beneficiary, &token_id, &total, &start, &0, &100, &true);

        env.ledger().set_timestamp(start.saturating_add(40));
        let paid = client.claim(&0);
        prop_assert!(paid <= total * 40 / 100 + 1);
        prop_assert_eq!(client.claimed(&0), paid);
        prop_assert!(client.claimed(&0) <= client.vested(&0));
    }
}
