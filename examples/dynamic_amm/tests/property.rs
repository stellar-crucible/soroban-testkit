extern crate std;

use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};
use soroban_testkit_example_dynamic_amm::{
    calculate_amount_out, calculate_dynamic_fee_bps, integer_sqrt, DynamicAmmContract,
    DynamicAmmContractClient, DEFAULT_BASE_FEE_BPS, DEFAULT_MAX_FEE_BPS, DEFAULT_MIN_FEE_BPS,
    DEFAULT_TWAP_WINDOW, DEFAULT_VOLATILITY_FACTOR,
};
use soroban_testkit_generators::strategies::token_amount;

const ONE: i128 = 10_000_000;

proptest! {
    /// Acceptance Criterion: Proptest proving swap fees strictly stay within
    /// defined minimum and maximum basis points across all possible price deviations.
    #[test]
    fn proptest_dynamic_fee_strictly_within_min_and_max_bounds(
        spot_price in 1i128..=10_000_000_000_000i128,
        twap_price in 1i128..=10_000_000_000_000i128,
        min_fee in 1u32..=100u32,
        base_fee_offset in 0u32..=200u32,
        max_fee_offset in 0u32..=2000u32,
        volatility_factor in 0u32..=20u32,
    ) {
        let base_fee = min_fee + base_fee_offset;
        let max_fee = base_fee + max_fee_offset;

        let fee = calculate_dynamic_fee_bps(
            spot_price,
            twap_price,
            base_fee,
            min_fee,
            max_fee,
            volatility_factor,
        );

        prop_assert!(
            fee >= min_fee,
            "Fee {} below min_fee_bps {}",
            fee,
            min_fee
        );
        prop_assert!(
            fee <= max_fee,
            "Fee {} above max_fee_bps {}",
            fee,
            max_fee
        );
    }

    #[test]
    fn proptest_fee_increases_monotonically_with_price_deviation(
        twap_price in 1_000_000i128..=100_000_000_000i128,
        dev1 in 0i128..=50_000_000_000i128,
        dev2_extra in 0i128..=50_000_000_000i128,
        volatility_factor in 1u32..=10u32,
    ) {
        let dev2 = dev1 + dev2_extra;
        let spot1 = twap_price + dev1;
        let spot2 = twap_price + dev2;

        let fee1 = calculate_dynamic_fee_bps(
            spot1,
            twap_price,
            DEFAULT_BASE_FEE_BPS,
            DEFAULT_MIN_FEE_BPS,
            DEFAULT_MAX_FEE_BPS,
            volatility_factor,
        );
        let fee2 = calculate_dynamic_fee_bps(
            spot2,
            twap_price,
            DEFAULT_BASE_FEE_BPS,
            DEFAULT_MIN_FEE_BPS,
            DEFAULT_MAX_FEE_BPS,
            volatility_factor,
        );

        prop_assert!(
            fee2 >= fee1,
            "Fee at deviation {} ({}) must be >= fee at deviation {} ({})",
            dev2,
            fee2,
            dev1,
            fee1
        );
    }

    #[test]
    fn proptest_zero_deviation_yields_base_fee(
        price in 1i128..=10_000_000_000_000i128,
        base_fee in 10u32..=100u32,
    ) {
        let fee = calculate_dynamic_fee_bps(
            price,
            price,
            base_fee,
            5,
            500,
            2,
        );
        prop_assert_eq!(fee, base_fee);
    }

    #[test]
    fn proptest_constant_product_k_invariant_never_decreases(
        reserve_in in 10_000i128..=1_000_000_000_000i128,
        reserve_out in 10_000i128..=1_000_000_000_000i128,
        amount_in in 1i128..=10_000_000_000i128,
        fee_bps in 1u32..=1000u32,
    ) {
        if let Some((amount_out, _)) = calculate_amount_out(amount_in, reserve_in, reserve_out, fee_bps) {
            let k_before = reserve_in * reserve_out;
            let new_in = reserve_in + amount_in;
            let new_out = reserve_out - amount_out;
            let k_after = new_in * new_out;

            prop_assert!(
                k_after >= k_before,
                "Invariant violated: k_after {} < k_before {}",
                k_after,
                k_before
            );
        }
    }

    #[test]
    fn proptest_amount_out_strictly_less_than_reserve_out(
        reserve_in in 10_000i128..=1_000_000_000_000i128,
        reserve_out in 10_000i128..=1_000_000_000_000i128,
        amount_in in 1i128..=1_000_000_000_000i128,
        fee_bps in 1u32..=500u32,
    ) {
        if let Some((amount_out, _)) = calculate_amount_out(amount_in, reserve_in, reserve_out, fee_bps) {
            prop_assert!(
                amount_out < reserve_out,
                "amount_out {} must be < reserve_out {}",
                amount_out,
                reserve_out
            );
        }
    }

    #[test]
    fn proptest_integer_sqrt_properties(
        y in 0i128..=1_000_000_000_000_000i128,
    ) {
        let r = integer_sqrt(y);
        prop_assert!(r >= 0);
        prop_assert!(r * r <= y);
        prop_assert!((r + 1) * (r + 1) > y);
    }

    #[test]
    fn proptest_pool_contract_swap_preserves_constant_product(
        raw_amount in token_amount(),
    ) {
        let swap_amount = (raw_amount % (50_000 * ONE)) + 100;
        let env = Env::default();
        env.mock_all_auths();

        let pool = env.register(DynamicAmmContract, ());
        let admin = Address::generate(&env);
        let token_a = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let token_b = env.register_stellar_asset_contract_v2(admin.clone()).address();
        let trader = Address::generate(&env);
        let lp = Address::generate(&env);

        StellarAssetClient::new(&env, &token_a).mint(&lp, &(100_000 * ONE));
        StellarAssetClient::new(&env, &token_b).mint(&lp, &(100_000 * ONE));
        StellarAssetClient::new(&env, &token_a).mint(&trader, &swap_amount);

        let client = DynamicAmmContractClient::new(&env, &pool);
        client.initialize(
            &admin,
            &token_a,
            &token_b,
            &DEFAULT_MIN_FEE_BPS,
            &DEFAULT_MAX_FEE_BPS,
            &DEFAULT_BASE_FEE_BPS,
            &DEFAULT_VOLATILITY_FACTOR,
            &DEFAULT_TWAP_WINDOW,
        );

        client.deposit(&lp, &(100_000 * ONE), &(100_000 * ONE), &1);
        let (res_a_before, res_b_before) = client.get_reserves();
        let k_before = res_a_before * res_b_before;

        client.swap(&trader, &token_a, &swap_amount, &1);

        let (res_a_after, res_b_after) = client.get_reserves();
        let k_after = res_a_after * res_b_after;

        prop_assert!(
            k_after >= k_before,
            "k_after {} < k_before {}",
            k_after,
            k_before
        );
    }
}
