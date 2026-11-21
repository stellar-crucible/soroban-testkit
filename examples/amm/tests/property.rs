//! soroban-testkit-generators + proptest: the constant-product invariant
//! under arbitrary trade sizes, against both the pure math and a live pool.

mod common;

use common::{Amm, ONE};
use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env};
use soroban_testkit_example_amm::{
    integer_sqrt, optimal_deposit, quote_amount_out, shares_for_deposit, DEFAULT_FEE_BPS,
};
use soroban_testkit_generators::strategies::{account_address, token_amount};

/// Reserves from 1 to 10^15: deep enough to exercise rounding on both ends,
/// shallow enough that `reserve * reserve` cannot overflow an `i128`.
fn reserve() -> impl Strategy<Value = i128> {
    token_amount().prop_map(|amount| amount % 1_000_000_000_000_000 + 1)
}

/// A trade from one stroop up to 10x the pool depth used by the live tests.
fn trade() -> impl Strategy<Value = i128> {
    token_amount().prop_map(|amount| amount % (1_000_000 * ONE) + 1)
}

fn fee() -> impl Strategy<Value = u32> {
    0u32..=1_000
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn a_quoted_swap_never_decreases_k(
        reserve_in in reserve(),
        reserve_out in reserve(),
        amount_in in reserve(),
        fee_bps in fee(),
    ) {
        let out = quote_amount_out(reserve_in, reserve_out, amount_in, fee_bps).unwrap();

        prop_assert!(out >= 0);
        prop_assert!(out < reserve_out, "a swap can never drain the output side");
        let k_before = reserve_in * reserve_out;
        let k_after = (reserve_in + amount_in) * (reserve_out - out);
        prop_assert!(k_after >= k_before, "k fell from {} to {}", k_before, k_after);
    }

    #[test]
    fn a_higher_fee_never_pays_out_more(
        reserve_in in reserve(),
        reserve_out in reserve(),
        amount_in in reserve(),
        low in 0u32..500,
        extra in 0u32..500,
    ) {
        let cheap = quote_amount_out(reserve_in, reserve_out, amount_in, low).unwrap();
        let dear = quote_amount_out(reserve_in, reserve_out, amount_in, low + extra).unwrap();

        prop_assert!(dear <= cheap);
    }

    #[test]
    fn integer_sqrt_is_the_floor_of_the_root(value in 0i128..=1_000_000_000_000_000_000_000_000_000_000i128) {
        let root = integer_sqrt(value);

        prop_assert!(root * root <= value);
        prop_assert!((root + 1) * (root + 1) > value);
    }

    #[test]
    fn an_optimal_deposit_never_exceeds_what_was_offered(
        reserve_a in reserve(),
        reserve_b in reserve(),
        desired_a in reserve(),
        desired_b in reserve(),
    ) {
        let (a, b) = optimal_deposit(reserve_a, reserve_b, desired_a, desired_b).unwrap();

        prop_assert!(a <= desired_a && b <= desired_b);
        prop_assert!(a == desired_a || b == desired_b, "one side is always taken in full");
    }

    #[test]
    fn deposit_shares_never_exceed_the_proportional_claim(
        reserve_a in reserve(),
        reserve_b in reserve(),
        total in reserve(),
        amount_a in reserve(),
        amount_b in reserve(),
    ) {
        let shares = shares_for_deposit(reserve_a, reserve_b, total, amount_a, amount_b).unwrap();

        // Claimed fraction of the pool never exceeds the deposited fraction.
        prop_assert!(shares * reserve_a <= amount_a * total);
        prop_assert!(shares * reserve_b <= amount_b * total);
    }

    #[test]
    fn generated_trader_addresses_are_usable_in_a_live_pool(bytes in account_address()) {
        let env = Env::default();
        env.mock_all_auths();
        let amm = Amm::deploy(&env, &Address::generate(&env), DEFAULT_FEE_BPS);
        let lp = Address::generate(&env);
        amm.fund(&lp, 1_000 * ONE);
        amm.pool().deposit(&lp, &(1_000 * ONE), &(1_000 * ONE), &0, &0);
        let trader = bytes.to_address(&env);
        amm.fund(&trader, ONE);

        let out = amm.pool().swap(&trader, &true, &ONE, &0);

        prop_assert_eq!(amm.token_a().balance(&trader), ONE + out);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// The live pool upholds `x * y >= k` across a sequence of arbitrary
    /// swaps in both directions.
    #[test]
    fn a_live_pool_keeps_k_non_decreasing_across_swaps(
        trades in prop::collection::vec((any::<bool>(), trade()), 1..8),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let amm = Amm::deploy(&env, &Address::generate(&env), DEFAULT_FEE_BPS);
        let lp = Address::generate(&env);
        let trader = Address::generate(&env);
        amm.fund(&lp, 100_000 * ONE);
        amm.fund(&trader, 100_000_000 * ONE);
        amm.pool().deposit(&lp, &(100_000 * ONE), &(100_000 * ONE), &0, &0);

        let mut k = amm.k();
        for (buy_a, amount) in trades {
            let quoted = amm.pool().quote(&buy_a, &amount);
            let out = amm.pool().swap(&trader, &buy_a, &amount, &0);
            prop_assert_eq!(out, quoted);
            let next = amm.k();
            prop_assert!(next >= k, "k fell from {} to {}", k, next);
            k = next;
        }
    }

    /// Depositing and immediately withdrawing never returns more than was put in.
    #[test]
    fn a_round_trip_through_the_pool_never_profits(
        seed_a in trade(),
        seed_b in trade(),
        deposit_a in trade(),
        deposit_b in trade(),
    ) {
        let env = Env::default();
        env.mock_all_auths();
        let amm = Amm::deploy(&env, &Address::generate(&env), DEFAULT_FEE_BPS);
        let seeder = Address::generate(&env);
        let lp = Address::generate(&env);
        amm.fund(&seeder, 10_000_000 * ONE);
        amm.fund(&lp, 10_000_000 * ONE);
        prop_assume!(integer_sqrt(seed_a * seed_b) > 10_000);
        amm.pool().deposit(&seeder, &seed_a, &seed_b, &0, &0);

        let Ok(Ok(shares)) = amm.pool().try_deposit(&lp, &deposit_a, &deposit_b, &0, &0) else {
            // Too small to mint a single share: refused, nothing moved.
            return Ok(());
        };
        let (a, b) = amm.pool().withdraw(&lp, &shares, &0, &0);

        prop_assert!(amm.token_a().balance(&lp) <= 10_000_000 * ONE);
        prop_assert!(amm.token_b().balance(&lp) <= 10_000_000 * ONE);
        prop_assert!(a <= deposit_a && b <= deposit_b);
    }
}
