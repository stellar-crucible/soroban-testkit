#![no_std]
#![allow(deprecated)]

pub mod debt_token;
pub mod oracle;
pub mod pool;
pub mod vault;

pub use debt_token::{DebtToken, DebtTokenClient, DebtTokenError};
pub use oracle::{LendingOracle, LendingOracleClient, OracleError, PriceData};
pub use pool::{
    BorrowingPool, BorrowingPoolClient, DebtPosition, LiquidationResult, PoolConfig, PoolError,
};
pub use vault::{CollateralVault, CollateralVaultClient, VaultConfig, VaultError, VaultPosition};

pub const PRICE_SCALE: i128 = 10_000_000;
pub const BPS_DENOMINATOR: i128 = 10_000;
pub const SECONDS_PER_YEAR: u64 = 31_536_000;

pub fn collateral_value(collateral: i128, price: i128) -> Option<i128> {
    collateral.checked_mul(price)?.checked_div(PRICE_SCALE)
}

pub fn max_borrow(collateral: i128, price: i128, debt: i128, max_ltv_bps: u32) -> Option<i128> {
    let ceiling = collateral_value(collateral, price)?
        .checked_mul(max_ltv_bps as i128)?
        .checked_div(BPS_DENOMINATOR)?;
    ceiling.checked_sub(debt)
}

pub fn health_factor_bps(collateral: i128, debt: i128, price: i128) -> Option<i128> {
    if debt <= 0 {
        return Some(i128::MAX);
    }
    collateral_value(collateral, price)?
        .checked_mul(BPS_DENOMINATOR)?
        .checked_div(debt)
}

pub fn is_healthy(collateral: i128, debt: i128, price: i128, threshold_bps: u32) -> Option<bool> {
    if debt <= 0 {
        return Some(true);
    }
    let lhs = collateral
        .checked_mul(price)?
        .checked_mul(BPS_DENOMINATOR)?;
    let rhs = debt
        .checked_mul(threshold_bps as i128)?
        .checked_mul(PRICE_SCALE)?;
    Some(lhs >= rhs)
}

pub fn accrued_interest(debt: i128, rate_bps: u32, elapsed: u64) -> Option<i128> {
    if debt <= 0 || rate_bps == 0 || elapsed == 0 {
        return Some(0);
    }
    debt.checked_mul(rate_bps as i128)?
        .checked_mul(elapsed as i128)?
        .checked_div(BPS_DENOMINATOR * SECONDS_PER_YEAR as i128)
}

pub fn collateral_seized(debt: i128, price: i128, penalty_bps: u32) -> Option<i128> {
    debt.checked_mul(BPS_DENOMINATOR + penalty_bps as i128)?
        .checked_div(BPS_DENOMINATOR)?
        .checked_mul(PRICE_SCALE)?
        .checked_div(price)
}
