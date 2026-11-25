//! Protocol invariants under generated inputs, with `proptest` and
//! `soroban-testkit-generators`: no borrow past the LTV ceiling, no reserved
//! collateral withdrawn, exact interest, and conservation across liquidation.

mod common;

use common::*;
use proptest::prelude::*;
use soroban_testkit_example_lending::{
    accrued_interest, health_factor_bps, is_healthy, max_borrow, PoolError, VaultError,
};
use soroban_testkit_generators::financial::{basis_points, stroop_amount};
use soroban_testkit_generators::strategies::token_amount;

/// One stroop up to an actor's whole funding.
fn collateral() -> impl Strategy<Value = i128> {
    stroop_amount().prop_map(|amount| 1 + amount % FUNDING)
}

/// Prices from $0.0001 to $10 per collateral unit.
fn price() -> impl Strategy<Value = i128> {
    1_000i128..=100_000_000i128
}

/// A positive borrow request in stroops.
fn borrow_amount() -> impl Strategy<Value = i128> {
    token_amount().prop_map(|amount| 1 + amount)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_a_borrow_never_exceeds_the_ltv_ceiling(
        collateral in collateral(),
        price in price(),
        amount in borrow_amount(),
    ) {
        let lending = Lending::new();
        lending.set_price(price);
        lending.deposit(&lending.owner, collateral);
        let ceiling = lending.pool().max_borrow(&lending.owner);

        match lending.pool().try_borrow(&lending.owner, &amount) {
            Ok(Ok(position)) => {
                prop_assert!(amount <= ceiling, "borrowed {amount} above ceiling {ceiling}");
                prop_assert_eq!(position.debt, amount);
                prop_assert_eq!(lending.debt().balance(&lending.owner), amount);
                prop_assert_eq!(lending.debt().total_supply(), amount);
                prop_assert_eq!(
                    lending.pool().health_factor(&lending.owner),
                    health_factor_bps(collateral, amount, price).unwrap()
                );
            }
            Ok(Err(error)) => prop_assert!(false, "undecodable borrow error: {error:?}"),
            Err(error) => {
                prop_assert_eq!(error.unwrap(), PoolError::AboveMaxLtv);
                prop_assert!(amount > ceiling);
                prop_assert_eq!(lending.debt().total_supply(), 0);
                prop_assert_eq!(lending.pool().debt_of(&lending.owner), 0);
            }
        }
    }

    #[test]
    fn prop_a_lower_price_never_raises_the_health_factor(
        collateral in collateral(),
        price in 2i128..=100_000_000i128,
        share in 1i128..=100,
    ) {
        let lending = Lending::new();
        lending.set_price(price);
        lending.deposit(&lending.owner, collateral);
        let debt = lending.pool().max_borrow(&lending.owner) * share / 100;
        prop_assume!(debt > 0);
        lending.borrow(&lending.owner, debt);

        let before = lending.pool().health_factor(&lending.owner);
        let lower = 1 + (price - 1) / 2;
        lending.set_price(lower);
        let after = lending.pool().health_factor(&lending.owner);

        prop_assert!(after <= before, "health rose from {before} to {after}");
        prop_assert_eq!(
            after,
            health_factor_bps(collateral, debt, lower).unwrap()
        );
    }

    #[test]
    fn prop_max_borrow_is_always_safe_to_take(
        collateral in collateral(),
        price in price(),
    ) {
        let lending = Lending::new();
        lending.set_price(price);
        lending.deposit(&lending.owner, collateral);
        let ceiling = lending.pool().max_borrow(&lending.owner);
        prop_assert_eq!(
            ceiling,
            max_borrow(collateral, price, 0, MAX_LTV_BPS).unwrap().max(0)
        );

        if ceiling > 0 {
            lending.borrow(&lending.owner, ceiling);
            prop_assert_eq!(lending.pool().max_borrow(&lending.owner), 0);
            prop_assert_eq!(lending.debt().balance(&lending.owner), ceiling);
            prop_assert!(
                lending.pool().health_factor(&lending.owner) >= MAX_LTV_BPS as i128
            );
        }
    }

    #[test]
    fn prop_withdrawal_never_releases_reserved_collateral(
        collateral in collateral(),
        debt_share in 1i128..=100,
        withdraw_share in 1i128..=200,
    ) {
        let lending = Lending::new();
        lending.deposit(&lending.owner, collateral);
        let debt = lending.pool().max_borrow(&lending.owner) * debt_share / 100;
        prop_assume!(debt > 0);
        lending.borrow(&lending.owner, debt);

        let required = lending.pool().required_collateral(&lending.owner);
        let amount = (collateral * withdraw_share / 100).max(1);
        let healthy_after = collateral - amount >= required;

        match lending.vault().try_withdraw(&lending.owner, &amount) {
            Ok(Ok(position)) => {
                prop_assert!(healthy_after, "withdrew reserved collateral");
                prop_assert!(position.collateral >= required);
            }
            Ok(Err(error)) => prop_assert!(false, "undecodable withdraw error: {error:?}"),
            Err(error) => {
                prop_assert!(!healthy_after, "refused a safe withdrawal of {amount}");
                let error = error.unwrap();
                prop_assert!(
                    error == VaultError::ReservedCollateral
                        || error == VaultError::InsufficientCollateral,
                    "unexpected error: {error:?}"
                );
                prop_assert_eq!(lending.vault().collateral_of(&lending.owner), collateral);
            }
        }
    }

    #[test]
    fn prop_interest_follows_simple_interest_at_any_rate(
        principal in borrow_amount(),
        rate in basis_points(),
        elapsed in 0u64..=5 * YEAR,
    ) {
        let mut lending = Lending::with_params(rate);
        // Cap the principal so it can be borrowed against the default fixture.
        let principal = principal % (8_000 * ONE) + 1;
        lending.open(&lending.owner, 10_000 * ONE, principal);
        lending.advance_time_without_oracle(elapsed);

        let position = lending.pool().accrue(&lending.owner);

        let expected = principal + accrued_interest(principal, rate, elapsed).unwrap();
        prop_assert_eq!(position.debt, expected);
        prop_assert!(position.debt >= principal);
        prop_assert_eq!(lending.pool().debt_of(&lending.owner), expected);
        prop_assert_eq!(lending.pool().total_debt(), expected);
        prop_assert_eq!(lending.debt().total_supply(), principal);
    }

    #[test]
    fn prop_liquidation_only_below_threshold_and_conserves_collateral(
        drop_percent in 0i128..=95,
    ) {
        let collateral = 10_000 * ONE;
        let debt = 6_000 * ONE;
        let lending = Lending::new();
        lending.open(&lending.owner, collateral, debt);
        lending.arm_liquidator(debt);
        let price = lending.crash_price(drop_percent);
        let vault_before = lending.collateral_token().balance(&lending.vault_id);
        let liquidator_before = lending.collateral_token().balance(&lending.liquidator);

        match lending.pool().try_liquidate(&lending.liquidator, &lending.owner) {
            Ok(Ok(result)) => {
                prop_assert!(!is_healthy(collateral, debt, price, THRESHOLD_BPS).unwrap());
                prop_assert_eq!(result.collateral_seized + result.collateral_remaining, collateral);
                prop_assert_eq!(result.debt_repaid, debt);
                prop_assert_eq!(
                    vault_before - lending.collateral_token().balance(&lending.vault_id),
                    result.collateral_seized
                );
                prop_assert_eq!(
                    lending.collateral_token().balance(&lending.liquidator) - liquidator_before,
                    result.collateral_seized
                );
                prop_assert_eq!(lending.pool().debt_of(&lending.owner), 0);
                prop_assert_eq!(lending.debt().balance(&lending.liquidator), 0);
            }
            Ok(Err(error)) => prop_assert!(false, "undecodable liquidate error: {error:?}"),
            Err(error) => {
                prop_assert_eq!(error.unwrap(), PoolError::NotLiquidatable);
                prop_assert_eq!(lending.pool().debt_of(&lending.owner), debt);
            }
        }
    }

    #[test]
    fn prop_two_borrowers_have_additive_totals(
        first in collateral(),
        second in collateral(),
        first_share in basis_points(),
        second_share in basis_points(),
    ) {
        let lending = Lending::new();
        lending.deposit(&lending.owner, first);
        lending.deposit(&lending.other, second);
        let first_debt = lending.pool().max_borrow(&lending.owner) * first_share as i128 / 10_000;
        let second_debt = lending.pool().max_borrow(&lending.other) * second_share as i128 / 10_000;
        prop_assume!(first_debt > 0 && second_debt > 0);
        lending.borrow(&lending.owner, first_debt);
        lending.borrow(&lending.other, second_debt);

        prop_assert_eq!(lending.pool().total_debt(), first_debt + second_debt);
        prop_assert_eq!(lending.debt().total_supply(), first_debt + second_debt);
        prop_assert_eq!(lending.vault().total_collateral(), first + second);
        prop_assert_eq!(lending.pool().debt_of(&lending.owner), first_debt);
        prop_assert_eq!(lending.pool().debt_of(&lending.other), second_debt);
        prop_assert_eq!(
            lending.vault().collateral_of(&lending.owner),
            first
        );
        prop_assert_eq!(
            lending.vault().collateral_of(&lending.other),
            second
        );
    }
}
