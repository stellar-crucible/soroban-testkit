//! A constant-product automated market maker, split across the contracts a
//! real deployment would have:
//!
//! * [`AmmToken`] — an ERC-20 shaped token. The example deploys it three
//!   times: the two pool assets, and the pool's LP share token (whose admin is
//!   the pool, so only the pool can mint shares).
//! * [`ConstantProductPool`] — holds reserves of two tokens, mints LP shares
//!   on deposit, burns them on withdrawal, and swaps along `x * y = k` with a
//!   fee that stays in the pool for liquidity providers.
//! * [`Router`] — the user-facing entry point that forwards a swap to a pool,
//!   which is what makes the authorization tree three levels deep:
//!   `router.swap_exact_in` → `pool.swap` → `token.transfer`.
//!
//! The arithmetic lives in free functions ([`quote_amount_out`],
//! [`optimal_deposit`], [`shares_for_deposit`], [`integer_sqrt`]) so property
//! tests can check it without a contract in the loop.
#![no_std]
// Events use the tuple-topic `publish` form the testkit event matchers read.
#![allow(deprecated)]

mod pool;
mod router;
mod token;

pub use pool::{AmmError, ConstantProductPool, ConstantProductPoolClient, PoolKey};
pub use router::{Router, RouterClient};
pub use token::{AmmToken, AmmTokenClient, TokenError, TokenKey};

/// Basis-point denominator: a fee of `30` is 0.30%.
pub const BPS: i128 = 10_000;

/// The fee the examples deploy pools with, in basis points.
pub const DEFAULT_FEE_BPS: u32 = 30;

/// Shares burned to the pool itself on the first deposit, so the share price
/// can never be pushed to an absurd value by a dust-sized first deposit.
pub const MINIMUM_LIQUIDITY: i128 = 1_000;

/// Floor of the square root of a non-negative `value`.
pub fn integer_sqrt(value: i128) -> i128 {
    if value <= 0 {
        return 0;
    }
    let mut x = value;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + value / x) / 2;
    }
    x
}

/// How much of the output token a swap of `amount_in` returns, after the fee.
///
/// `None` when an input is non-positive or the arithmetic would overflow.
/// The result always leaves `(reserve_in + amount_in) * (reserve_out - out)`
/// at or above `reserve_in * reserve_out`.
pub fn quote_amount_out(
    reserve_in: i128,
    reserve_out: i128,
    amount_in: i128,
    fee_bps: u32,
) -> Option<i128> {
    if reserve_in <= 0 || reserve_out <= 0 || amount_in <= 0 {
        return None;
    }
    let in_with_fee = amount_in.checked_mul(BPS - fee_bps as i128)?;
    let numerator = in_with_fee.checked_mul(reserve_out)?;
    let denominator = reserve_in.checked_mul(BPS)?.checked_add(in_with_fee)?;
    Some(numerator / denominator)
}

/// The amounts a deposit actually moves so it matches the pool's ratio,
/// given the most the depositor is willing to send of each token.
///
/// An empty pool takes both desired amounts and sets the price.
pub fn optimal_deposit(
    reserve_a: i128,
    reserve_b: i128,
    desired_a: i128,
    desired_b: i128,
) -> Option<(i128, i128)> {
    if reserve_a == 0 && reserve_b == 0 {
        return Some((desired_a, desired_b));
    }
    let b_for_a = desired_a.checked_mul(reserve_b)? / reserve_a;
    if b_for_a <= desired_b {
        return Some((desired_a, b_for_a));
    }
    let a_for_b = desired_b.checked_mul(reserve_a)? / reserve_b;
    Some((a_for_b, desired_b))
}

/// LP shares minted for depositing `amount_a` and `amount_b`.
///
/// The first deposit mints `sqrt(a * b)` (of which [`MINIMUM_LIQUIDITY`] is
/// locked); later deposits mint in proportion to the smaller side, so a
/// lopsided deposit cannot dilute existing holders.
pub fn shares_for_deposit(
    reserve_a: i128,
    reserve_b: i128,
    total_shares: i128,
    amount_a: i128,
    amount_b: i128,
) -> Option<i128> {
    if total_shares == 0 {
        return Some(integer_sqrt(amount_a.checked_mul(amount_b)?));
    }
    let by_a = amount_a.checked_mul(total_shares)? / reserve_a;
    let by_b = amount_b.checked_mul(total_shares)? / reserve_b;
    Some(by_a.min(by_b))
}
