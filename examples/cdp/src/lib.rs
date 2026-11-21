//! A synthetic asset protocol built from three cooperating contracts, plus a
//! collateral token:
//!
//! * [`PriceOracle`](oracle::PriceOracle) publishes the USD price of the
//!   collateral asset.
//! * [`SyntheticToken`](synth::SyntheticToken) is the synthetic USD (`sUSD`)
//!   that only the engine may mint or burn.
//! * [`CdpEngine`](engine::CdpEngine) holds each owner's collateral, issues
//!   debt against it subject to a minimum collateralization ratio (MCR),
//!   accrues a stability fee on outstanding debt, and lets anyone liquidate a
//!   position that has fallen below the MCR.
//!
//! Collateral is any Soroban token. The tests use a Stellar Asset Contract to
//! stand in for XLM.
//!
//! Each contract lives in its own module so the generated clients and specs
//! never collide. A deployment builds each contract as its own WASM.
#![no_std]
#![allow(deprecated)]

pub mod engine;
pub mod oracle;
pub mod synth;

pub use engine::{CdpConfig, CdpEngine, CdpEngineClient, CdpError, LiquidationResult, Position};
pub use oracle::{OracleError, PriceData, PriceOracle, PriceOracleClient};
pub use synth::{SynthError, SyntheticToken, SyntheticTokenClient};

/// Fixed-point scale of oracle prices: a price of `PRICE_SCALE` means 1 USD per
/// unit of collateral. Matches the 7 decimals Stellar assets use.
pub const PRICE_SCALE: i128 = 10_000_000;

/// Denominator for every ratio and rate expressed in basis points.
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Seconds in a 365-day year: the period a stability fee rate is quoted over.
pub const SECONDS_PER_YEAR: u64 = 31_536_000;

/// The USD value of `collateral` units at `price`, in synthetic-asset units.
///
/// `None` on overflow.
pub fn collateral_value(collateral: i128, price: i128) -> Option<i128> {
    collateral.checked_mul(price)?.checked_div(PRICE_SCALE)
}

/// The collateralization ratio in basis points, or `i128::MAX` when there is no
/// debt (an unindebted position is infinitely safe).
///
/// `None` on overflow.
pub fn collateral_ratio_bps(collateral: i128, debt: i128, price: i128) -> Option<i128> {
    if debt <= 0 {
        return Some(i128::MAX);
    }
    collateral_value(collateral, price)?
        .checked_mul(BPS_DENOMINATOR)?
        .checked_div(debt)
}

/// Whether `collateral` at `price` backs `debt` at or above `mcr_bps`.
///
/// Compared by cross-multiplication rather than through
/// [`collateral_ratio_bps`], so integer division never rounds a position that
/// is a hair below the MCR up to exactly the MCR. `None` on overflow.
pub fn is_healthy(collateral: i128, debt: i128, price: i128, mcr_bps: u32) -> Option<bool> {
    if debt <= 0 {
        return Some(true);
    }
    let lhs = collateral
        .checked_mul(price)?
        .checked_mul(BPS_DENOMINATOR)?;
    let rhs = debt
        .checked_mul(mcr_bps as i128)?
        .checked_mul(PRICE_SCALE)?;
    Some(lhs >= rhs)
}

/// Simple-interest stability fee accrued on `debt` over `elapsed` seconds at
/// `fee_bps` per year, rounded down. `None` on overflow.
pub fn accrued_fee(debt: i128, fee_bps: u32, elapsed: u64) -> Option<i128> {
    if debt <= 0 || fee_bps == 0 || elapsed == 0 {
        return Some(0);
    }
    debt.checked_mul(fee_bps as i128)?
        .checked_mul(elapsed as i128)?
        .checked_div(BPS_DENOMINATOR * SECONDS_PER_YEAR as i128)
}
