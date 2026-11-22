//! Protocol invariants under randomized inputs, generated with
//! `soroban-testkit-generators` and `proptest`. The central property: synthetic
//! assets can never be minted against a position that would end up below the
//! minimum collateralization ratio.

mod common;

use common::*;
use proptest::prelude::*;
use soroban_testkit_example_cdp::{accrued_fee, is_healthy, CdpError};
use soroban_testkit_generators::strategies::token_amount;

/// Collateral between one stroop and the whole funding of an actor.
fn collateral() -> impl Strategy<Value = i128> {
    token_amount().prop_map(|amount| 1 + amount % FUNDING)
}

/// Prices from $0.0001 to $10 per XLM.
fn price() -> impl Strategy<Value = i128> {
    1_000i128..=100_000_000i128
}

/// Borrow requests from one stroop up to 1,000,000 sUSD.
fn borrow_amount() -> impl Strategy<Value = i128> {
    token_amount().prop_map(|amount| 1 + amount % (1_000_000 * ONE))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A borrow succeeds exactly when the resulting position is at or above
    /// the MCR, and a refused borrow mints nothing.
    #[test]
    fn prop_synthetic_is_never_minted_below_the_mcr(
        collateral in collateral(),
        price in price(),
        amount in borrow_amount(),
    ) {
        let cdp = Cdp::new();
        cdp.set_price(price);
        cdp.engine().deposit(&cdp.owner, &collateral);

        let healthy = is_healthy(collateral, amount, price, MCR_BPS).unwrap();
        match cdp.engine().try_borrow(&cdp.owner, &amount) {
            Ok(Ok(position)) => {
                prop_assert!(healthy, "minted {amount} against {collateral} at {price}");
                prop_assert!(is_healthy(position.collateral, position.debt, price, MCR_BPS).unwrap());
                prop_assert!(cdp.engine().collateral_ratio(&cdp.owner) >= MCR_BPS as i128);
                prop_assert_eq!(cdp.synth().total_supply(), amount);
            }
            Ok(Err(_)) => prop_assert!(false, "borrow returned an undecodable result"),
            Err(err) => {
                prop_assert!(!healthy, "refused a healthy borrow of {amount} against {collateral} at {price}");
                prop_assert_eq!(err.unwrap(), CdpError::BelowMinCollateralRatio);
                prop_assert_eq!(cdp.synth().total_supply(), 0);
                prop_assert_eq!(cdp.engine().position(&cdp.owner).debt, 0);
            }
        }
    }

    /// However a sequence of borrows and price moves interleaves, every borrow
    /// that went through left the position healthy at the price it was made
    /// at, and supply equals the sum of accepted borrows.
    #[test]
    fn prop_borrow_sequences_respect_the_mcr_at_every_step(
        collateral in collateral(),
        steps in prop::collection::vec((price(), borrow_amount()), 1..6),
    ) {
        let cdp = Cdp::new();
        cdp.engine().deposit(&cdp.owner, &collateral);
        let mut minted = 0i128;

        for (price, amount) in steps {
            cdp.set_price(price);
            if cdp.engine().try_borrow(&cdp.owner, &amount).is_ok() {
                minted += amount;
                let position = cdp.engine().position(&cdp.owner);
                prop_assert!(is_healthy(position.collateral, position.debt, price, MCR_BPS).unwrap());
            }
            prop_assert_eq!(cdp.synth().total_supply(), minted);
            prop_assert_eq!(cdp.engine().total_debt(), minted);
        }
    }

    /// `max_borrow` is always safe to borrow in full.
    #[test]
    fn prop_max_borrow_is_always_mintable(collateral in collateral(), price in price()) {
        let cdp = Cdp::new();
        cdp.set_price(price);
        cdp.engine().deposit(&cdp.owner, &collateral);

        let max = cdp.engine().max_borrow(&cdp.owner);
        if max > 0 {
            cdp.engine().borrow(&cdp.owner, &max);
            prop_assert!(cdp.engine().collateral_ratio(&cdp.owner) >= MCR_BPS as i128);
            prop_assert_eq!(cdp.engine().max_borrow(&cdp.owner), 0);
        }
    }

    /// Withdrawing collateral can never take an indebted position below the
    /// MCR either.
    #[test]
    fn prop_withdraw_never_leaves_a_position_below_the_mcr(
        collateral in collateral(),
        debt_share in 1i128..=100,
        withdraw_share in 1i128..=100,
    ) {
        let cdp = Cdp::new();
        cdp.engine().deposit(&cdp.owner, &collateral);
        let debt = cdp.engine().max_borrow(&cdp.owner) * debt_share / 100;
        prop_assume!(debt > 0);
        cdp.engine().borrow(&cdp.owner, &debt);

        let amount = (collateral * withdraw_share / 100).max(1);
        let healthy_after = is_healthy(collateral - amount, debt, XLM_PRICE, MCR_BPS).unwrap();
        let result = cdp.engine().try_withdraw(&cdp.owner, &amount);

        prop_assert_eq!(result.is_ok(), healthy_after);
        let position = cdp.engine().position(&cdp.owner);
        prop_assert!(is_healthy(position.collateral, position.debt, XLM_PRICE, MCR_BPS).unwrap());
    }

    /// Liquidation is possible exactly when a position is below the MCR, and
    /// it splits the position's collateral between liquidator and owner
    /// without creating or losing any.
    #[test]
    fn prop_liquidation_only_below_the_mcr_and_conserves_collateral(
        drop_percent in 0i128..=95,
    ) {
        let cdp = Cdp::new();
        cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
        cdp.arm_liquidator(600 * ONE);
        let price = cdp.crash_price(drop_percent);
        let engine_before = cdp.xlm().balance(&cdp.engine_id);
        let liquidator_before = cdp.xlm().balance(&cdp.liquidator);

        let healthy = is_healthy(10_000 * ONE, 600 * ONE, price, MCR_BPS).unwrap();
        match cdp.engine().try_liquidate(&cdp.liquidator, &cdp.owner) {
            Ok(Ok(result)) => {
                prop_assert!(!healthy);
                prop_assert_eq!(result.collateral_seized + result.collateral_remaining, 10_000 * ONE);
                prop_assert_eq!(cdp.engine().position(&cdp.owner).debt, 0);
                prop_assert_eq!(
                    cdp.xlm().balance(&cdp.liquidator) - liquidator_before,
                    engine_before - cdp.xlm().balance(&cdp.engine_id)
                );
            }
            Ok(Err(_)) => prop_assert!(false, "liquidate returned an undecodable result"),
            Err(err) => {
                prop_assert!(healthy);
                prop_assert_eq!(err.unwrap(), CdpError::NotLiquidatable);
            }
        }
    }

    /// Accrued debt follows the simple-interest formula for any rate and
    /// elapsed time, and never decreases.
    #[test]
    fn prop_stability_fee_follows_simple_interest(
        debt in 1i128..=600,
        fee_bps in 0u32..=2_000,
        elapsed in 0u64..=5 * YEAR,
    ) {
        let mut cdp = Cdp::with_fee(fee_bps);
        let principal = debt * ONE;
        cdp.open(&cdp.owner, 10_000 * ONE, principal);

        cdp.advance_time_without_oracle(elapsed);
        let position = cdp.engine().accrue(&cdp.owner);

        let expected = principal + accrued_fee(principal, fee_bps, elapsed).unwrap();
        prop_assert_eq!(position.debt, expected);
        prop_assert!(position.debt >= principal);
        prop_assert_eq!(cdp.engine().total_debt(), expected);
        prop_assert_eq!(cdp.synth().total_supply(), principal);
    }
}
