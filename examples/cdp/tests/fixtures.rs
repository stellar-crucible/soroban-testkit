//! Protocol lifecycle driven through `soroban-testkit-fixtures`: collateral
//! locking, synthetic minting against the MCR, repayment, stability fee
//! accrual on a moving ledger clock, oracle staleness, and liquidation.

mod common;

use common::*;
use soroban_testkit_example_cdp::{CdpEngine, CdpEngineClient, CdpError, LiquidationResult};
use soroban_testkit_fixtures::TestContext;

#[test]
fn initialize_wires_the_engine_to_its_token_oracle_and_collateral() {
    let cdp = Cdp::new();

    let config = cdp.engine().config();
    assert_eq!(config.collateral_token, cdp.collateral);
    assert_eq!(config.synth_token, cdp.synth_id);
    assert_eq!(config.oracle, cdp.oracle_id);
    assert_eq!(config.mcr_bps, MCR_BPS);
    assert_eq!(cdp.synth().minter(), cdp.engine_id);
    assert_eq!(cdp.engine().total_debt(), 0);
    assert_eq!(cdp.engine().total_collateral(), 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let cdp = Cdp::new();

    let err = cdp
        .engine()
        .try_initialize(&cdp.config(MCR_BPS, PENALTY_BPS, FEE_BPS));
    assert_eq!(err.unwrap_err().unwrap(), CdpError::AlreadyInitialized);
}

#[test]
fn an_mcr_at_or_below_100_percent_is_rejected() {
    let cdp = Cdp::deployed();

    for mcr in [0, 5_000, 10_000] {
        let err = cdp
            .engine()
            .try_initialize(&cdp.config(mcr, PENALTY_BPS, FEE_BPS));
        assert_eq!(err.unwrap_err().unwrap(), CdpError::InvalidParameter);
    }
}

#[test]
fn an_uninitialized_engine_rejects_deposits() {
    let ctx = TestContext::new();
    let engine_id = ctx.env.register(CdpEngine, ());
    let engine = CdpEngineClient::new(&ctx.env, &engine_id);

    let err = engine.try_deposit(&ctx.admin, &ONE);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::NotInitialized);
}

#[test]
fn deposit_locks_collateral_in_the_engine() {
    let cdp = Cdp::new();

    let position = cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    assert_eq!(position.collateral, 10_000 * ONE);
    assert_eq!(position.debt, 0);
    assert_eq!(cdp.xlm().balance(&cdp.owner), FUNDING - 10_000 * ONE);
    assert_eq!(cdp.xlm().balance(&cdp.engine_id), 10_000 * ONE);
    assert_eq!(cdp.engine().total_collateral(), 10_000 * ONE);
}

#[test]
fn a_non_positive_deposit_is_rejected() {
    let cdp = Cdp::new();

    for amount in [0, -ONE] {
        let err = cdp.engine().try_deposit(&cdp.owner, &amount);
        assert_eq!(err.unwrap_err().unwrap(), CdpError::InvalidAmount);
    }
}

#[test]
fn borrow_mints_synthetic_usd_against_collateral() {
    let cdp = Cdp::new();

    // 10,000 XLM at $0.10 = $1,000 of collateral; 600 sUSD is a 166% ratio.
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);

    assert_eq!(cdp.synth().balance(&cdp.owner), 600 * ONE);
    assert_eq!(cdp.synth().total_supply(), 600 * ONE);
    assert_eq!(cdp.engine().position(&cdp.owner).debt, 600 * ONE);
    assert_eq!(cdp.engine().collateral_ratio(&cdp.owner), 16_666);
}

#[test]
fn borrowing_exactly_max_borrow_succeeds_and_one_more_unit_fails() {
    let cdp = Cdp::new();
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    let max = cdp.engine().max_borrow(&cdp.owner);
    // $1,000 / 150% = $666.6666666 (truncated to 7 decimals)
    assert_eq!(max, 6_666_666_666);

    let err = cdp.engine().try_borrow(&cdp.owner, &(max + 1));
    assert_eq!(err.unwrap_err().unwrap(), CdpError::BelowMinCollateralRatio);

    cdp.engine().borrow(&cdp.owner, &max);
    assert_eq!(cdp.engine().max_borrow(&cdp.owner), 0);
    assert!(cdp.engine().collateral_ratio(&cdp.owner) >= MCR_BPS as i128);
}

#[test]
fn borrowing_without_a_position_is_rejected() {
    let cdp = Cdp::new();

    let err = cdp.engine().try_borrow(&cdp.owner, &ONE);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::NoPosition);
}

#[test]
fn withdraw_is_blocked_when_it_would_breach_the_mcr() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);

    // 600 sUSD at 150% needs $900 = 9,000 XLM; 1,000 XLM is free.
    cdp.engine().withdraw(&cdp.owner, &(1_000 * ONE));
    let err = cdp.engine().try_withdraw(&cdp.owner, &ONE);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::BelowMinCollateralRatio);
    assert_eq!(cdp.engine().position(&cdp.owner).collateral, 9_000 * ONE);
}

#[test]
fn withdrawing_everything_from_a_debt_free_position_closes_it() {
    let cdp = Cdp::new();
    cdp.engine().deposit(&cdp.owner, &(500 * ONE));

    let position = cdp.engine().withdraw(&cdp.owner, &(500 * ONE));

    assert_eq!(position.collateral, 0);
    assert_eq!(cdp.xlm().balance(&cdp.owner), FUNDING);
    let err = cdp.engine().try_withdraw(&cdp.owner, &ONE);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::NoPosition);
}

#[test]
fn repay_burns_synthetic_and_reduces_debt() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);

    let position = cdp.engine().repay(&cdp.owner, &(200 * ONE));

    assert_eq!(position.debt, 400 * ONE);
    assert_eq!(cdp.synth().balance(&cdp.owner), 400 * ONE);
    assert_eq!(cdp.synth().total_supply(), 400 * ONE);
    assert_eq!(cdp.engine().total_debt(), 400 * ONE);

    let err = cdp.engine().try_repay(&cdp.owner, &(400 * ONE + 1));
    assert_eq!(err.unwrap_err().unwrap(), CdpError::ExceedsDebt);
}

#[test]
fn stability_fee_accrues_two_percent_over_a_year() {
    let mut cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);

    cdp.advance_time(YEAR);

    // Pending, but not yet written to the position.
    assert_eq!(cdp.engine().debt_of(&cdp.owner), 612 * ONE);
    assert_eq!(cdp.engine().position(&cdp.owner).debt, 600 * ONE);

    let position = cdp.engine().accrue(&cdp.owner);
    assert_eq!(position.debt, 612 * ONE);
    assert_eq!(position.last_accrual, cdp.now());
    assert_eq!(cdp.engine().total_debt(), 612 * ONE);
    // Fees are owed, not minted: supply still tracks what was borrowed.
    assert_eq!(cdp.synth().total_supply(), 600 * ONE);
}

#[test]
fn accrued_fees_are_owed_on_repayment() {
    let mut cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.advance_time(YEAR / 2);

    // Half a year at 2% = 6 sUSD of fees; repaying the principal leaves it.
    let position = cdp.engine().repay(&cdp.owner, &(600 * ONE));
    assert_eq!(position.debt, 6 * ONE);
}

#[test]
fn fee_accrual_pushes_a_borderline_position_below_the_mcr() {
    let mut cdp = Cdp::with_fee(1_000); // 10% a year
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));
    let max = cdp.engine().max_borrow(&cdp.owner);
    cdp.engine().borrow(&cdp.owner, &max);
    assert!(!cdp.engine().is_liquidatable(&cdp.owner));

    cdp.advance_time(YEAR / 12);

    assert!(cdp.engine().is_liquidatable(&cdp.owner));
    assert!(cdp.engine().collateral_ratio(&cdp.owner) < MCR_BPS as i128);
}

#[test]
fn a_stale_price_blocks_borrowing() {
    let mut cdp = Cdp::new();
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    cdp.advance_time_without_oracle(MAX_PRICE_AGE + 1);

    let err = cdp.engine().try_borrow(&cdp.owner, &ONE);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::StalePrice);
}

#[test]
fn a_missing_price_is_reported_as_unavailable() {
    let cdp = Cdp::deployed();
    cdp.engine()
        .initialize(&cdp.config(MCR_BPS, PENALTY_BPS, FEE_BPS));
    cdp.fund(&cdp.owner, FUNDING);
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    let err = cdp.engine().try_borrow(&cdp.owner, &ONE);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::PriceUnavailable);
}

#[test]
fn a_healthy_position_cannot_be_liquidated() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(600 * ONE);

    let err = cdp.engine().try_liquidate(&cdp.liquidator, &cdp.owner);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::NotLiquidatable);
}

#[test]
fn a_price_drop_makes_a_position_liquidatable_with_a_penalty_bonus() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(600 * ONE);
    let liquidator_xlm = cdp.xlm().balance(&cdp.liquidator);

    // $0.08: collateral worth $800 backs 600 sUSD at 133% < 150%.
    cdp.crash_price(20);
    let result = cdp.engine().liquidate(&cdp.liquidator, &cdp.owner);

    // 600 sUSD + 10% = $660 of XLM at $0.08 = 8,250 XLM.
    assert_eq!(
        result,
        LiquidationResult {
            debt_repaid: 600 * ONE,
            collateral_seized: 8_250 * ONE,
            collateral_remaining: 1_750 * ONE,
        }
    );
    assert_eq!(
        cdp.xlm().balance(&cdp.liquidator),
        liquidator_xlm + 8_250 * ONE
    );
    assert_eq!(cdp.synth().balance(&cdp.liquidator), 0);
    let position = cdp.engine().position(&cdp.owner);
    assert_eq!(position.debt, 0);
    assert_eq!(position.collateral, 1_750 * ONE);

    // The owner keeps the minted sUSD and can reclaim the leftover collateral.
    cdp.engine().withdraw(&cdp.owner, &(1_750 * ONE));
    assert_eq!(cdp.synth().balance(&cdp.owner), 600 * ONE);
}

#[test]
fn an_underwater_position_hands_the_liquidator_all_its_collateral() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(600 * ONE);

    // $0.05: collateral worth $500 is less than the 600 sUSD owed.
    cdp.crash_price(50);
    let result = cdp.engine().liquidate(&cdp.liquidator, &cdp.owner);

    assert_eq!(result.collateral_seized, 10_000 * ONE);
    assert_eq!(result.collateral_remaining, 0);
    assert_eq!(cdp.engine().position(&cdp.owner).collateral, 0);
}

#[test]
fn a_liquidator_without_enough_synthetic_is_rejected() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(100 * ONE);
    cdp.crash_price(20);

    let err = cdp.engine().try_liquidate(&cdp.liquidator, &cdp.owner);
    assert_eq!(err.unwrap_err().unwrap(), CdpError::InsufficientSynth);
    assert_eq!(cdp.engine().position(&cdp.owner).debt, 600 * ONE);
}

#[test]
fn positions_are_isolated_between_owners() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.open(&cdp.other, 50_000 * ONE, 1_000 * ONE);

    cdp.crash_price(20);

    assert!(cdp.engine().is_liquidatable(&cdp.owner));
    assert!(!cdp.engine().is_liquidatable(&cdp.other));
    assert_eq!(cdp.engine().total_debt(), 1_600 * ONE);
    assert_eq!(cdp.engine().total_collateral(), 60_000 * ONE);
}
