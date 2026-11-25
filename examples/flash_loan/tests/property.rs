//! soroban-testkit-generators + proptest: fee precision over arbitrary borrow
//! amounts and fee rates, and end-to-end loan invariants.

mod common;

use common::{Market, LIQUIDITY};
use proptest::prelude::*;
use soroban_sdk::testutils::Address as _;
use soroban_testkit_example_flash_loan::{
    compute_fee, FlashLoanError, RepayMode, BPS_DENOMINATOR, MAX_FEE_BPS,
};
use soroban_testkit_generators::financial::basis_points;
use soroban_testkit_generators::strategies::token_amount;

/// Fee rates the provider accepts.
fn fee_rate() -> impl Strategy<Value = u32> {
    basis_points().prop_map(|bps| bps % (MAX_FEE_BPS + 1))
}

proptest! {
    /// The fee is the exact ceiling of `amount * bps / 10_000`.
    #[test]
    fn fee_is_the_exact_ceiling(amount in token_amount(), bps in fee_rate()) {
        let fee = compute_fee(amount, bps).expect("token amounts never overflow");
        let exact = amount * bps as i128;
        prop_assert!(fee * BPS_DENOMINATOR >= exact);
        prop_assert!(fee == 0 || (fee - 1) * BPS_DENOMINATOR < exact);
    }

    /// Borrowing more never costs less.
    #[test]
    fn fee_is_monotonic_in_the_amount(
        amount in token_amount(),
        extra in 0i128..1_000_000,
        bps in fee_rate(),
    ) {
        prop_assert!(compute_fee(amount + extra, bps).unwrap() >= compute_fee(amount, bps).unwrap());
    }

    /// A positive loan is never free unless the rate is zero, and the fee never
    /// exceeds the principal.
    #[test]
    fn fee_is_bounded(amount in 1i128..=1_000_000_000_000i128, bps in fee_rate()) {
        let fee = compute_fee(amount, bps).unwrap();
        prop_assert!(fee <= amount);
        prop_assert_eq!(fee == 0, bps == 0);
    }

    /// Negative amounts and overflowing products are refused, not wrapped.
    #[test]
    fn out_of_range_inputs_have_no_fee(amount in i128::MIN..0, bps in fee_rate()) {
        prop_assert_eq!(compute_fee(amount, bps), None);
        prop_assert_eq!(compute_fee(i128::MAX, MAX_FEE_BPS), None);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// Any affordable loan succeeds and grows the pool by exactly its fee.
    #[test]
    fn any_repaid_loan_grows_the_pool_by_its_fee(amount in 1i128..=LIQUIDITY) {
        let market = Market::setup();
        let expected = market.lender().flash_fee(&amount);

        let fee = market.borrow(0, amount);

        prop_assert_eq!(fee, expected);
        prop_assert_eq!(market.balance(0, &market.provider), LIQUIDITY + fee);
        prop_assert_eq!(market.lender().fees_collected(&market.tokens[0]), fee);
    }

    /// Any shortfall, however small, rolls the loan back.
    #[test]
    fn any_shortfall_rolls_back(amount in 1i128..=1_000_000_000i128, shortfall in 1i128..=1_000) {
        let market = Market::setup();
        market.set_mode(RepayMode::ShortBy(shortfall));

        let outcome = market.lender().try_flash_loan(
            &market.borrower,
            &market.receiver,
            &market.tokens[0],
            &amount,
            &market.data(),
        );

        prop_assert_eq!(outcome.unwrap_err().unwrap(), FlashLoanError::RepaymentShort);
        prop_assert_eq!(market.balance(0, &market.provider), LIQUIDITY);
        prop_assert_eq!(market.lender().loan_count(), 0);
    }

    /// Fees attributed to borrowers always add up to the token total.
    #[test]
    fn attribution_adds_up(loans in proptest::collection::vec((any::<bool>(), 1i128..=1_000_000), 1..6)) {
        let market = Market::setup();
        let other = soroban_sdk::Address::generate(&market.env);
        let lender = market.lender();
        let data = market.data();
        let mut total = 0;
        for (first, amount) in loans {
            let borrower = if first { &market.borrower } else { &other };
            total += lender.flash_loan(borrower, &market.receiver, &market.tokens[0], &amount, &data);
        }
        prop_assert_eq!(lender.fees_collected(&market.tokens[0]), total);
        prop_assert_eq!(
            lender.borrower_fees(&market.borrower, &market.tokens[0])
                + lender.borrower_fees(&other, &market.tokens[0]),
            total
        );
    }
}
