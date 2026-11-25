mod common;

use common::*;
use soroban_testkit_example_lending::{
    accrued_interest, collateral_value, health_factor_bps, is_healthy, max_borrow, DebtToken,
    DebtTokenClient, LendingOracle, LendingOracleClient, OracleError, PoolError, VaultError,
};

#[test]
fn initialize_wires_vault_pool_oracle_and_debt_token() {
    let lending = Lending::new();

    let vault = lending.vault().config();
    let pool = lending.pool().config();
    assert_eq!(vault.pool, lending.pool_id);
    assert_eq!(vault.collateral_token, lending.collateral);
    assert_eq!(pool.vault, lending.vault_id);
    assert_eq!(pool.oracle, lending.oracle_id);
    assert_eq!(pool.debt_token, lending.debt_id);
    assert_eq!(pool.max_ltv_bps, MAX_LTV_BPS);
    assert_eq!(pool.liquidation_threshold_bps, THRESHOLD_BPS);
    assert_eq!(lending.debt().minter(), lending.pool_id);
    assert_eq!(lending.vault().total_collateral(), 0);
    assert_eq!(lending.pool().total_debt(), 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let lending = Lending::new();

    assert_eq!(
        lending
            .vault()
            .try_initialize(&soroban_testkit_example_lending::VaultConfig {
                pool: lending.pool_id.clone(),
                collateral_token: lending.collateral.clone(),
            })
            .unwrap_err()
            .unwrap(),
        VaultError::AlreadyInitialized
    );
    assert_eq!(
        lending
            .pool()
            .try_initialize(&lending.config(INTEREST_BPS))
            .unwrap_err()
            .unwrap(),
        PoolError::AlreadyInitialized
    );
}

#[test]
fn invalid_pool_parameters_are_rejected() {
    let lending = Lending::deployed();
    let mut config = lending.config(INTEREST_BPS);
    config.max_ltv_bps = 10_000;
    assert_eq!(
        lending.pool().try_initialize(&config).unwrap_err().unwrap(),
        PoolError::InvalidParameter
    );

    let mut config = lending.config(INTEREST_BPS);
    config.liquidation_threshold_bps = 8_000;
    assert_eq!(
        lending.pool().try_initialize(&config).unwrap_err().unwrap(),
        PoolError::InvalidParameter
    );
}

#[test]
fn an_uninitialized_vault_rejects_deposits() {
    let lending = Lending::deployed();

    assert_eq!(
        lending
            .vault()
            .try_deposit(&lending.owner, &ONE)
            .unwrap_err()
            .unwrap(),
        VaultError::NotInitialized
    );
}

#[test]
fn deposit_locks_collateral_in_the_vault() {
    let lending = Lending::new();

    let position = lending.vault().deposit(&lending.owner, &(10_000 * ONE));

    assert_eq!(position.collateral, 10_000 * ONE);
    assert_eq!(
        lending.collateral_token().balance(&lending.owner),
        FUNDING - 10_000 * ONE
    );
    assert_eq!(
        lending.collateral_token().balance(&lending.vault_id),
        10_000 * ONE
    );
    assert_eq!(lending.vault().total_collateral(), 10_000 * ONE);
}

#[test]
fn a_non_positive_deposit_is_rejected() {
    let lending = Lending::new();

    for amount in [0, -ONE] {
        assert_eq!(
            lending
                .vault()
                .try_deposit(&lending.owner, &amount)
                .unwrap_err()
                .unwrap(),
            VaultError::InvalidAmount
        );
    }
}

#[test]
fn borrowing_within_max_ltv_mints_debt_tokens() {
    let lending = Lending::new();
    lending.deposit(&lending.owner, 10_000 * ONE);

    lending.borrow(&lending.owner, 6_000 * ONE);

    assert_eq!(lending.debt().balance(&lending.owner), 6_000 * ONE);
    assert_eq!(lending.debt().total_supply(), 6_000 * ONE);
    assert_eq!(lending.pool().debt_of(&lending.owner), 6_000 * ONE);
    assert_eq!(lending.pool().health_factor(&lending.owner), 16_666);
    assert!(is_healthy(10_000 * ONE, 6_000 * ONE, PRICE, THRESHOLD_BPS).unwrap());
}

#[test]
fn borrowing_exactly_the_ceiling_succeeds_and_one_more_stroop_fails() {
    let lending = Lending::new();
    lending.deposit(&lending.owner, 10_000 * ONE);
    let ceiling = lending.pool().max_borrow(&lending.owner);
    assert_eq!(ceiling, 8_000 * ONE);

    assert_eq!(
        lending
            .pool()
            .try_borrow(&lending.owner, &(ceiling + 1))
            .unwrap_err()
            .unwrap(),
        PoolError::AboveMaxLtv
    );
    lending.borrow(&lending.owner, ceiling);
    assert_eq!(lending.pool().max_borrow(&lending.owner), 0);
    assert_eq!(lending.pool().health_factor(&lending.owner), 12_500);
}

#[test]
fn borrowing_without_collateral_is_rejected() {
    let lending = Lending::new();

    assert_eq!(
        lending
            .pool()
            .try_borrow(&lending.owner, &ONE)
            .unwrap_err()
            .unwrap(),
        PoolError::NoCollateral
    );
}

#[test]
fn withdraw_is_blocked_when_collateral_is_reserved_for_debt() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    assert_eq!(
        lending.pool().required_collateral(&lending.owner),
        6_000 * ONE
    );

    lending.vault().withdraw(&lending.owner, &(4_000 * ONE));
    assert_eq!(
        lending
            .vault()
            .try_withdraw(&lending.owner, &ONE)
            .unwrap_err()
            .unwrap(),
        VaultError::ReservedCollateral
    );
    assert_eq!(lending.vault().collateral_of(&lending.owner), 6_000 * ONE);
}

#[test]
fn repayment_frees_collateral_for_withdrawal() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);

    lending.pool().repay(&lending.owner, &(6_000 * ONE));
    assert_eq!(lending.debt().balance(&lending.owner), 0);
    assert_eq!(lending.debt().total_supply(), 0);
    assert_eq!(lending.pool().debt_of(&lending.owner), 0);
    assert_eq!(lending.pool().required_collateral(&lending.owner), 0);

    let position = lending.vault().withdraw(&lending.owner, &(10_000 * ONE));
    assert_eq!(position.collateral, 0);
    assert_eq!(lending.vault().total_collateral(), 0);
    assert_eq!(lending.collateral_token().balance(&lending.owner), FUNDING);
}

#[test]
fn interest_accrues_over_ledger_time() {
    let mut lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);

    lending.advance_time(YEAR);
    assert_eq!(lending.pool().debt_of(&lending.owner), 6_300 * ONE);

    let position = lending.pool().accrue(&lending.owner);
    assert_eq!(position.debt, 6_300 * ONE);
    assert_eq!(lending.pool().total_debt(), 6_300 * ONE);
    assert_eq!(lending.debt().total_supply(), 6_000 * ONE);
    assert_eq!(
        accrued_interest(6_000 * ONE, INTEREST_BPS, YEAR),
        Some(300 * ONE)
    );
}

#[test]
fn a_price_drop_lowers_the_health_factor() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    let before = lending.pool().health_factor(&lending.owner);

    lending.set_price(6_500_000);
    let after = lending.pool().health_factor(&lending.owner);

    assert_eq!(before, 16_666);
    assert_eq!(after, 10_833);
    assert!(after < THRESHOLD_BPS as i128);
    assert!(lending.pool().is_liquidatable(&lending.owner));
    assert_eq!(
        health_factor_bps(10_000 * ONE, 6_000 * ONE, 6_500_000),
        Some(10_833)
    );
}

#[test]
fn a_third_party_can_liquidate_and_seize_collateral() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.arm_liquidator(6_000 * ONE);
    let liquidator_before = lending.collateral_token().balance(&lending.liquidator);
    lending.set_price(6_500_000);
    assert!(lending.pool().is_liquidatable(&lending.owner));

    let result = lending
        .pool()
        .liquidate(&lending.liquidator, &lending.owner);

    assert_eq!(result.debt_repaid, 6_000 * ONE);
    assert!(result.collateral_seized > 0);
    assert!(result.collateral_remaining > 0);
    assert_eq!(
        result.collateral_seized + result.collateral_remaining,
        10_000 * ONE
    );
    assert_eq!(
        lending.vault().collateral_of(&lending.owner),
        result.collateral_remaining
    );
    assert_eq!(
        lending.collateral_token().balance(&lending.liquidator),
        liquidator_before + result.collateral_seized
    );
    assert_eq!(lending.pool().debt_of(&lending.owner), 0);
    assert_eq!(lending.debt().balance(&lending.liquidator), 0);
}

#[test]
fn a_healthy_position_cannot_be_liquidated() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);

    assert_eq!(
        lending
            .pool()
            .try_liquidate(&lending.liquidator, &lending.owner)
            .unwrap_err()
            .unwrap(),
        PoolError::NotLiquidatable
    );
}

#[test]
fn two_borrowers_have_isolated_positions() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.open(&lending.other, 20_000 * ONE, 1_000 * ONE);

    assert_eq!(lending.pool().debt_of(&lending.owner), 6_000 * ONE);
    assert_eq!(lending.pool().debt_of(&lending.other), 1_000 * ONE);
    assert_eq!(lending.vault().collateral_of(&lending.owner), 10_000 * ONE);
    assert_eq!(lending.vault().collateral_of(&lending.other), 20_000 * ONE);
    assert_eq!(lending.pool().total_debt(), 7_000 * ONE);
    assert_eq!(lending.vault().total_collateral(), 30_000 * ONE);
    assert_eq!(lending.debt().total_supply(), 7_000 * ONE);
}

#[test]
fn pure_math_helpers_match_contract_queries() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);

    assert_eq!(collateral_value(10_000 * ONE, PRICE), Some(10_000 * ONE));
    assert_eq!(
        max_borrow(10_000 * ONE, PRICE, 6_000 * ONE, MAX_LTV_BPS),
        Some(2_000 * ONE)
    );
    assert_eq!(lending.pool().max_borrow(&lending.owner), 2_000 * ONE);
    assert_eq!(accrued_interest(6_000 * ONE, 0, YEAR), Some(0));
    assert_eq!(health_factor_bps(10_000 * ONE, 0, PRICE), Some(i128::MAX));
}

#[test]
fn oracle_rejects_a_non_positive_price() {
    let lending = Lending::new();

    for price in [0, -ONE] {
        assert_eq!(
            lending.oracle().try_set_price(&price).unwrap_err().unwrap(),
            OracleError::InvalidPrice
        );
    }
}

#[test]
fn deploying_token_contracts_twice_is_rejected() {
    let lending = Lending::deployed();
    let env = &lending.ctx.env;
    let oracle_id = env.register(LendingOracle, ());
    let debt_id = env.register(DebtToken, ());
    let oracle = LendingOracleClient::new(env, &oracle_id);
    let debt = DebtTokenClient::new(env, &debt_id);
    oracle.initialize(&lending.ctx.admin);
    debt.initialize(&lending.pool_id);

    assert_eq!(
        oracle
            .try_initialize(&lending.ctx.admin)
            .unwrap_err()
            .unwrap(),
        OracleError::AlreadyInitialized
    );
    assert_eq!(
        debt.try_initialize(&lending.pool_id).unwrap_err().unwrap(),
        soroban_testkit_example_lending::DebtTokenError::AlreadyInitialized
    );
}
