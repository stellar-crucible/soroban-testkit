//! Pool invariants under randomized inputs, generated with
//! `soroban-testkit-generators` and `proptest`. The central property: claim
//! payouts can never exceed the active reserves, however many claims arrive and
//! however large the losses are.

mod common;

use common::*;
use proptest::prelude::*;
use soroban_testkit_example_insurance::{
    premium_for, ClaimStatus, BPS_DENOMINATOR, SECONDS_PER_YEAR,
};
use soroban_testkit_generators::strategies::token_amount;

/// Capital between one stroop and 50,000 units.
fn capital() -> impl Strategy<Value = i128> {
    token_amount().prop_map(|amount| 1 + amount % (50_000 * ONE))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Random claim frequency and loss sizes can never drain more than the
    /// pool holds. Every payout is bounded by the reserves captured immediately
    /// before settlement, and the lifetime total stays inside the capital plus
    /// premiums the pool took in.
    #[test]
    fn prop_payouts_never_exceed_the_reserves(
        capital in capital(),
        rate_bps in 0u32..=2_000,
        losses in prop::collection::vec(1i128..=10_000i128, 1..=6),
    ) {
        let insurance = Insurance::with_config(rate_bps, QUORUM, MAX_COVERAGE);
        insurance.deposit(&insurance.provider, capital);
        insurance.fund(&insurance.holder, 10_000_000 * ONE);

        for loss in losses {
            let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
            let claim = insurance.file(&insurance.holder, policy.id, loss * ONE);
            insurance.approve(claim.id);
            let reserves_before = insurance.pool().reserves();
            if reserves_before <= 0 {
                break;
            }

            let settled = insurance.settle(claim.id);

            prop_assert_eq!(settled.status, ClaimStatus::Settled);
            prop_assert!(settled.payout > 0);
            prop_assert!(settled.payout <= loss * ONE);
            prop_assert!(settled.payout <= reserves_before);
            prop_assert_eq!(
                insurance.pool().reserves(),
                reserves_before - settled.payout
            );
            prop_assert!(
                insurance.pool().total_payouts()
                    <= insurance.pool().total_deposited() + insurance.pool().total_premiums()
            );
        }

        prop_assert!(insurance.pool().reserves() >= 0);
        prop_assert_eq!(
            insurance.token().balance(&insurance.pool_id),
            insurance.pool().reserves()
        );
    }

    /// Whatever the deposit order and size, the issued shares are exactly the
    /// shares the providers hold, and the reserves are exactly the capital
    /// deposited behind them.
    #[test]
    fn prop_shares_track_every_deposit(
        deposits in prop::collection::vec(capital(), 1..=8),
    ) {
        let insurance = Insurance::new();
        let mut expected = 0i128;

        for (index, amount) in deposits.iter().enumerate() {
            let provider = if index % 2 == 0 {
                &insurance.provider
            } else {
                &insurance.provider2
            };
            insurance.deposit(provider, *amount);
            expected += *amount;

            prop_assert_eq!(
                insurance.pool().total_shares(),
                insurance.pool().shares_of(&insurance.provider)
                    + insurance.pool().shares_of(&insurance.provider2)
            );
            prop_assert_eq!(insurance.pool().reserves(), expected);
            prop_assert_eq!(insurance.token().balance(&insurance.pool_id), expected);
        }
    }

    /// A withdrawal pays a proportional slice of what the pool holds at that
    /// moment, so it can neither overdraw the reserves nor mint value.
    #[test]
    fn prop_withdraw_never_takes_more_than_the_reserves(
        amount in capital(),
        share_bps in 1u32..=10_000,
    ) {
        let insurance = Insurance::new();
        let shares = insurance.deposit(&insurance.provider, amount);
        let request = (shares * share_bps as i128 / BPS_DENOMINATOR).max(1).min(shares);
        let reserves = insurance.pool().reserves();

        let paid = insurance.pool().withdraw(&insurance.provider, &request);

        prop_assert!(paid > 0);
        prop_assert!(paid <= reserves);
        prop_assert_eq!(insurance.pool().reserves(), reserves - paid);
        prop_assert!(insurance.pool().reserves() >= 0);
        prop_assert_eq!(
            insurance.pool().total_shares(),
            insurance.pool().shares_of(&insurance.provider)
                + insurance.pool().shares_of(&insurance.provider2)
        );
    }

    /// The premium is simple interest: coverage times rate times the fraction
    /// of a year the policy runs, rounded up, and never decreasing in duration.
    #[test]
    fn prop_premium_is_simple_interest_on_the_coverage(
        coverage in 1i128..=1_000_000i128,
        rate_bps in 0u32..=10_000,
        duration in 1u64..=5 * YEAR,
    ) {
        let units = coverage * ONE;
        let premium = premium_for(units, rate_bps, duration).unwrap();
        let denominator = BPS_DENOMINATOR * SECONDS_PER_YEAR as i128;

        let numerator = units * rate_bps as i128 * duration as i128;
        // Ceiling division in stable Rust (`i128::div_ceil` is unstable).
        let expected = if numerator == 0 {
            0
        } else {
            (numerator + denominator - 1) / denominator
        };
        prop_assert_eq!(premium, expected);
        prop_assert!(premium >= 0);
        prop_assert!(premium >= premium_for(units, rate_bps, duration / 2).unwrap());
        prop_assert!(
            premium <= units * rate_bps as i128 * 5 / BPS_DENOMINATOR + 1
        );
    }

    /// Only an approving quorum can pay: any vote pattern that ends rejected
    /// leaves the reserves and the lifetime payouts untouched.
    #[test]
    fn prop_only_an_approving_quorum_pays(
        votes in prop::collection::vec(any::<bool>(), 3..=3),
    ) {
        let insurance = Insurance::new();
        insurance.deposit(&insurance.provider, 1_000_000 * ONE);
        let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
        let reserves = insurance.pool().reserves();

        // Stop once quorum resolves the claim: a later vote is refused, and
        // the property is about the outcome, not about voting past it.
        for (index, approve) in votes.iter().enumerate() {
            if insurance
                .pool()
                .try_vote(&insurance.assessors[index], &claim.id, approve)
                .is_err()
            {
                break;
            }
        }

        let resolved = insurance.pool().claim(&claim.id);
        if resolved.status == ClaimStatus::Approved {
            let settled = insurance.settle(claim.id);
            prop_assert_eq!(settled.status, ClaimStatus::Settled);
            prop_assert!(settled.payout <= reserves);
            prop_assert!(settled.payout <= 8_000 * ONE);
            prop_assert_eq!(
                insurance.pool().reserves(),
                reserves - settled.payout
            );
        } else {
            prop_assert_eq!(resolved.status, ClaimStatus::Rejected);
            prop_assert!(insurance.pool().try_settle(&claim.id).is_err());
            prop_assert_eq!(insurance.pool().total_payouts(), 0);
            prop_assert_eq!(insurance.pool().reserves(), reserves);
        }
    }
}
