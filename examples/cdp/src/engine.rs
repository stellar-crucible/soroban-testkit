use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env,
};

use crate::oracle::PriceOracleClient;
use crate::synth::SyntheticTokenClient;
use crate::{accrued_fee, collateral_ratio_bps, collateral_value, is_healthy};
use crate::{BPS_DENOMINATOR, PRICE_SCALE};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum CdpError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidAmount = 3,
    InvalidParameter = 4,
    NoPosition = 5,
    InsufficientCollateral = 6,
    /// The operation would leave the position below the minimum
    /// collateralization ratio.
    BelowMinCollateralRatio = 7,
    ExceedsDebt = 8,
    /// The position is at or above the MCR, so it cannot be liquidated.
    NotLiquidatable = 9,
    PriceUnavailable = 10,
    StalePrice = 11,
    /// The payer does not hold enough of the synthetic asset.
    InsufficientSynth = 12,
    Overflow = 13,
}

/// Protocol parameters, fixed at initialization.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CdpConfig {
    pub admin: Address,
    /// The token locked as collateral (XLM's Stellar Asset Contract in tests).
    pub collateral_token: Address,
    /// The synthetic asset this engine mints, which must name the engine as
    /// its minter.
    pub synth_token: Address,
    pub oracle: Address,
    /// Minimum collateralization ratio in basis points (`15_000` = 150%).
    /// Borrowing and withdrawing must keep a position at or above it, and a
    /// position below it may be liquidated.
    pub mcr_bps: u32,
    /// Bonus collateral a liquidator receives on top of the debt it repays, in
    /// basis points of that debt.
    pub liquidation_penalty_bps: u32,
    /// Annual simple-interest stability fee on outstanding debt, in basis points.
    pub stability_fee_bps: u32,
    /// The oldest price, in seconds, the engine will act on.
    pub max_price_age: u64,
}

/// One owner's vault: collateral locked, synthetic debt owed, and when the
/// stability fee was last added to that debt.
#[contracttype]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Position {
    pub collateral: i128,
    pub debt: i128,
    pub last_accrual: u64,
}

/// What a liquidation did.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidationResult {
    /// Synthetic debt the liquidator burned (the position's whole debt).
    pub debt_repaid: i128,
    /// Collateral transferred to the liquidator.
    pub collateral_seized: i128,
    /// Collateral left in the position for its owner to withdraw.
    pub collateral_remaining: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum CdpKey {
    Config,
    TotalDebt,
    TotalCollateral,
    Position(Address),
}

/// Holds collateral, issues synthetic debt against it, accrues stability fees,
/// and liquidates undercollateralized positions.
#[contract]
pub struct CdpEngine;

#[contractimpl]
impl CdpEngine {
    pub fn initialize(env: Env, config: CdpConfig) -> Result<(), CdpError> {
        if env.storage().instance().has(&CdpKey::Config) {
            return Err(CdpError::AlreadyInitialized);
        }
        if config.mcr_bps as i128 <= BPS_DENOMINATOR
            || config.liquidation_penalty_bps as i128 > BPS_DENOMINATOR
            || config.stability_fee_bps as i128 > BPS_DENOMINATOR
            || config.max_price_age == 0
        {
            return Err(CdpError::InvalidParameter);
        }
        env.storage().instance().set(&CdpKey::Config, &config);
        env.storage().instance().set(&CdpKey::TotalDebt, &0i128);
        env.storage()
            .instance()
            .set(&CdpKey::TotalCollateral, &0i128);
        Ok(())
    }

    /// Lock `amount` of collateral into `owner`'s position.
    pub fn deposit(env: Env, owner: Address, amount: i128) -> Result<Position, CdpError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(CdpError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).unwrap_or_default();
        accrue(&env, &config, &owner, &mut position)?;

        token::Client::new(&env, &config.collateral_token).transfer(
            &owner,
            env.current_contract_address(),
            &amount,
        );
        position.collateral = position
            .collateral
            .checked_add(amount)
            .ok_or(CdpError::Overflow)?;
        adjust_total(&env, CdpKey::TotalCollateral, amount)?;
        save_position(&env, &owner, &position);
        env.events()
            .publish((symbol_short!("deposit"), owner), amount);
        Ok(position)
    }

    /// Release `amount` of collateral back to `owner`, provided the position
    /// stays at or above the MCR.
    pub fn withdraw(env: Env, owner: Address, amount: i128) -> Result<Position, CdpError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(CdpError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).ok_or(CdpError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        if amount > position.collateral {
            return Err(CdpError::InsufficientCollateral);
        }
        let remaining = position.collateral - amount;
        if position.debt > 0 {
            let price = fresh_price(&env, &config)?;
            if !is_healthy(remaining, position.debt, price, config.mcr_bps)
                .ok_or(CdpError::Overflow)?
            {
                return Err(CdpError::BelowMinCollateralRatio);
            }
        }

        position.collateral = remaining;
        adjust_total(&env, CdpKey::TotalCollateral, -amount)?;
        save_position(&env, &owner, &position);
        token::Client::new(&env, &config.collateral_token).transfer(
            &env.current_contract_address(),
            &owner,
            &amount,
        );
        env.events()
            .publish((symbol_short!("withdraw"), owner), amount);
        Ok(position)
    }

    /// Mint `amount` of the synthetic asset to `owner` as new debt, provided
    /// the position stays at or above the MCR.
    pub fn borrow(env: Env, owner: Address, amount: i128) -> Result<Position, CdpError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(CdpError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).ok_or(CdpError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;

        let new_debt = position
            .debt
            .checked_add(amount)
            .ok_or(CdpError::Overflow)?;
        let price = fresh_price(&env, &config)?;
        if !is_healthy(position.collateral, new_debt, price, config.mcr_bps)
            .ok_or(CdpError::Overflow)?
        {
            return Err(CdpError::BelowMinCollateralRatio);
        }

        position.debt = new_debt;
        adjust_total(&env, CdpKey::TotalDebt, amount)?;
        save_position(&env, &owner, &position);
        SyntheticTokenClient::new(&env, &config.synth_token).mint(&owner, &amount);
        env.events()
            .publish((symbol_short!("borrow"), owner), amount);
        Ok(position)
    }

    /// Burn `amount` of `owner`'s synthetic asset against their debt.
    pub fn repay(env: Env, owner: Address, amount: i128) -> Result<Position, CdpError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(CdpError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).ok_or(CdpError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        if amount > position.debt {
            return Err(CdpError::ExceedsDebt);
        }

        burn_synth(&env, &config, &owner, amount)?;
        position.debt -= amount;
        adjust_total(&env, CdpKey::TotalDebt, -amount)?;
        save_position(&env, &owner, &position);
        env.events()
            .publish((symbol_short!("repay"), owner), amount);
        Ok(position)
    }

    /// Close an undercollateralized position: `liquidator` burns the owner's
    /// whole debt and receives collateral worth that debt plus the liquidation
    /// penalty, capped at the collateral the position holds.
    pub fn liquidate(
        env: Env,
        liquidator: Address,
        owner: Address,
    ) -> Result<LiquidationResult, CdpError> {
        liquidator.require_auth();
        let config = load_config(&env)?;
        let mut position = load_position(&env, &owner).ok_or(CdpError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        if position.debt <= 0 {
            return Err(CdpError::NotLiquidatable);
        }
        let price = fresh_price(&env, &config)?;
        if is_healthy(position.collateral, position.debt, price, config.mcr_bps)
            .ok_or(CdpError::Overflow)?
        {
            return Err(CdpError::NotLiquidatable);
        }

        let debt = position.debt;
        let owed_value = debt
            .checked_mul(BPS_DENOMINATOR + config.liquidation_penalty_bps as i128)
            .ok_or(CdpError::Overflow)?
            / BPS_DENOMINATOR;
        let owed_collateral = owed_value
            .checked_mul(PRICE_SCALE)
            .ok_or(CdpError::Overflow)?
            / price;
        let seized = owed_collateral.min(position.collateral);

        burn_synth(&env, &config, &liquidator, debt)?;
        position.debt = 0;
        position.collateral -= seized;
        adjust_total(&env, CdpKey::TotalDebt, -debt)?;
        adjust_total(&env, CdpKey::TotalCollateral, -seized)?;
        save_position(&env, &owner, &position);
        if seized > 0 {
            token::Client::new(&env, &config.collateral_token).transfer(
                &env.current_contract_address(),
                &liquidator,
                &seized,
            );
        }

        let result = LiquidationResult {
            debt_repaid: debt,
            collateral_seized: seized,
            collateral_remaining: position.collateral,
        };
        env.events().publish(
            (symbol_short!("liquidate"), owner, liquidator),
            (debt, seized),
        );
        Ok(result)
    }

    /// Add the stability fee accrued so far to `owner`'s debt. Anyone may call
    /// it; it only brings stored state up to date.
    pub fn accrue(env: Env, owner: Address) -> Result<Position, CdpError> {
        let config = load_config(&env)?;
        let mut position = load_position(&env, &owner).ok_or(CdpError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        save_position(&env, &owner, &position);
        Ok(position)
    }

    /// The stored position, without fees accrued since its last update.
    pub fn position(env: Env, owner: Address) -> Position {
        load_position(&env, &owner).unwrap_or_default()
    }

    /// The debt `owner` would owe if their position were touched now,
    /// including the stability fee accrued since its last update.
    pub fn debt_of(env: Env, owner: Address) -> Result<i128, CdpError> {
        let config = load_config(&env)?;
        let position = load_position(&env, &owner).unwrap_or_default();
        current_debt(&env, &config, &position)
    }

    /// The position's collateralization ratio in basis points at the current
    /// price, counting pending fees. `i128::MAX` for a position without debt.
    pub fn collateral_ratio(env: Env, owner: Address) -> Result<i128, CdpError> {
        let config = load_config(&env)?;
        let position = load_position(&env, &owner).unwrap_or_default();
        let debt = current_debt(&env, &config, &position)?;
        if debt == 0 {
            return Ok(i128::MAX);
        }
        let price = fresh_price(&env, &config)?;
        collateral_ratio_bps(position.collateral, debt, price).ok_or(CdpError::Overflow)
    }

    /// How much more synthetic asset `owner` could borrow right now without
    /// falling below the MCR.
    pub fn max_borrow(env: Env, owner: Address) -> Result<i128, CdpError> {
        let config = load_config(&env)?;
        let position = load_position(&env, &owner).unwrap_or_default();
        let debt = current_debt(&env, &config, &position)?;
        let price = fresh_price(&env, &config)?;
        let ceiling = collateral_value(position.collateral, price)
            .and_then(|value| value.checked_mul(BPS_DENOMINATOR))
            .ok_or(CdpError::Overflow)?
            / config.mcr_bps as i128;
        Ok((ceiling - debt).max(0))
    }

    /// Whether the position could be liquidated right now.
    pub fn is_liquidatable(env: Env, owner: Address) -> Result<bool, CdpError> {
        let config = load_config(&env)?;
        let position = load_position(&env, &owner).unwrap_or_default();
        let debt = current_debt(&env, &config, &position)?;
        if debt == 0 {
            return Ok(false);
        }
        let price = fresh_price(&env, &config)?;
        Ok(
            !is_healthy(position.collateral, debt, price, config.mcr_bps)
                .ok_or(CdpError::Overflow)?,
        )
    }

    pub fn config(env: Env) -> Result<CdpConfig, CdpError> {
        load_config(&env)
    }

    pub fn total_debt(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&CdpKey::TotalDebt)
            .unwrap_or(0)
    }

    pub fn total_collateral(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&CdpKey::TotalCollateral)
            .unwrap_or(0)
    }
}

fn load_config(env: &Env) -> Result<CdpConfig, CdpError> {
    env.storage()
        .instance()
        .get(&CdpKey::Config)
        .ok_or(CdpError::NotInitialized)
}

fn load_position(env: &Env, owner: &Address) -> Option<Position> {
    env.storage()
        .persistent()
        .get(&CdpKey::Position(owner.clone()))
}

/// Store a position, or drop its entry once it holds nothing.
fn save_position(env: &Env, owner: &Address, position: &Position) {
    let key = CdpKey::Position(owner.clone());
    if position.collateral == 0 && position.debt == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, position);
    }
}

fn adjust_total(env: &Env, key: CdpKey, delta: i128) -> Result<(), CdpError> {
    let current: i128 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current.checked_add(delta).ok_or(CdpError::Overflow)?;
    env.storage().instance().set(&key, &next);
    Ok(())
}

/// The debt including the fee accrued since `position.last_accrual`.
fn current_debt(env: &Env, config: &CdpConfig, position: &Position) -> Result<i128, CdpError> {
    let elapsed = env
        .ledger()
        .timestamp()
        .saturating_sub(position.last_accrual);
    let fee =
        accrued_fee(position.debt, config.stability_fee_bps, elapsed).ok_or(CdpError::Overflow)?;
    position.debt.checked_add(fee).ok_or(CdpError::Overflow)
}

/// Fold the accrued stability fee into the position's debt.
///
/// The accrual clock only moves when a whole unit of fee was added (or when
/// there is no debt to charge), so a small position touched often still pays:
/// the fractional remainder keeps accumulating instead of being rounded away.
fn accrue(
    env: &Env,
    config: &CdpConfig,
    owner: &Address,
    position: &mut Position,
) -> Result<(), CdpError> {
    let now = env.ledger().timestamp();
    if position.debt == 0 {
        position.last_accrual = now;
        return Ok(());
    }
    let elapsed = now.saturating_sub(position.last_accrual);
    let fee =
        accrued_fee(position.debt, config.stability_fee_bps, elapsed).ok_or(CdpError::Overflow)?;
    if fee > 0 {
        position.debt = position.debt.checked_add(fee).ok_or(CdpError::Overflow)?;
        position.last_accrual = now;
        adjust_total(env, CdpKey::TotalDebt, fee)?;
        env.events()
            .publish((symbol_short!("fee"), owner.clone()), fee);
    }
    Ok(())
}

/// The oracle price, rejected when missing or older than `max_price_age`.
fn fresh_price(env: &Env, config: &CdpConfig) -> Result<i128, CdpError> {
    let data = match PriceOracleClient::new(env, &config.oracle).try_price() {
        Ok(Ok(data)) => data,
        _ => return Err(CdpError::PriceUnavailable),
    };
    let age = env.ledger().timestamp().saturating_sub(data.timestamp);
    if age > config.max_price_age {
        return Err(CdpError::StalePrice);
    }
    Ok(data.price)
}

fn burn_synth(env: &Env, config: &CdpConfig, from: &Address, amount: i128) -> Result<(), CdpError> {
    match SyntheticTokenClient::new(env, &config.synth_token).try_burn(from, &amount) {
        Ok(Ok(())) => Ok(()),
        _ => Err(CdpError::InsufficientSynth),
    }
}
