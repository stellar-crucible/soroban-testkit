use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

use crate::debt_token::DebtTokenClient;
use crate::oracle::LendingOracleClient;
use crate::vault::CollateralVaultClient;
use crate::{
    accrued_interest, collateral_seized, health_factor_bps, is_healthy, max_borrow, BPS_DENOMINATOR,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum PoolError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidAmount = 3,
    InvalidParameter = 4,
    NoPosition = 5,
    NoCollateral = 6,
    AboveMaxLtv = 7,
    ExceedsDebt = 8,
    NotLiquidatable = 9,
    PriceUnavailable = 10,
    StalePrice = 11,
    InsufficientDebtToken = 12,
    Overflow = 13,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolConfig {
    pub admin: Address,
    pub vault: Address,
    pub oracle: Address,
    pub debt_token: Address,
    pub max_ltv_bps: u32,
    pub liquidation_threshold_bps: u32,
    pub liquidation_penalty_bps: u32,
    pub interest_rate_bps: u32,
    pub max_price_age: u64,
}

#[contracttype]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DebtPosition {
    pub debt: i128,
    pub last_accrual: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiquidationResult {
    pub debt_repaid: i128,
    pub collateral_seized: i128,
    pub collateral_remaining: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum PoolKey {
    Config,
    TotalDebt,
    Position(Address),
}

#[contract]
pub struct BorrowingPool;

#[contractimpl]
impl BorrowingPool {
    pub fn initialize(env: Env, config: PoolConfig) -> Result<(), PoolError> {
        if env.storage().instance().has(&PoolKey::Config) {
            return Err(PoolError::AlreadyInitialized);
        }
        if config.max_ltv_bps == 0
            || config.max_ltv_bps as i128 >= BPS_DENOMINATOR
            || config.liquidation_threshold_bps <= config.max_ltv_bps
            || config.liquidation_threshold_bps > 20_000
            || config.liquidation_penalty_bps as i128 > BPS_DENOMINATOR
            || config.interest_rate_bps as i128 > BPS_DENOMINATOR
            || config.max_price_age == 0
        {
            return Err(PoolError::InvalidParameter);
        }
        env.storage().instance().set(&PoolKey::Config, &config);
        env.storage().instance().set(&PoolKey::TotalDebt, &0i128);
        Ok(())
    }

    pub fn borrow(env: Env, owner: Address, amount: i128) -> Result<DebtPosition, PoolError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(PoolError::InvalidAmount);
        }
        let collateral = CollateralVaultClient::new(&env, &config.vault).collateral_of(&owner);
        if collateral <= 0 {
            return Err(PoolError::NoCollateral);
        }
        let mut position = load_position(&env, &owner).unwrap_or_default();
        accrue(&env, &config, &owner, &mut position)?;
        let new_debt = position
            .debt
            .checked_add(amount)
            .ok_or(PoolError::Overflow)?;
        let price = fresh_price(&env, &config)?;
        if max_borrow(collateral, price, new_debt, config.max_ltv_bps).ok_or(PoolError::Overflow)?
            < 0
        {
            return Err(PoolError::AboveMaxLtv);
        }
        position.debt = new_debt;
        if position.last_accrual == 0 {
            position.last_accrual = env.ledger().timestamp();
        }
        adjust_total(&env, amount)?;
        save_position(&env, &owner, &position);
        DebtTokenClient::new(&env, &config.debt_token).mint(&owner, &amount);
        env.events()
            .publish((symbol_short!("borrow"), owner), amount);
        Ok(position)
    }

    pub fn repay(env: Env, owner: Address, amount: i128) -> Result<DebtPosition, PoolError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(PoolError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).ok_or(PoolError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        if amount > position.debt {
            return Err(PoolError::ExceedsDebt);
        }
        burn_debt(&env, &config, &owner, amount)?;
        position.debt -= amount;
        if position.debt == 0 {
            position.last_accrual = 0;
        }
        adjust_total(&env, -amount)?;
        save_position(&env, &owner, &position);
        env.events()
            .publish((symbol_short!("repay"), owner), amount);
        Ok(position)
    }

    pub fn liquidate(
        env: Env,
        liquidator: Address,
        owner: Address,
    ) -> Result<LiquidationResult, PoolError> {
        liquidator.require_auth();
        let config = load_config(&env)?;
        let mut position = load_position(&env, &owner).ok_or(PoolError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        if position.debt <= 0 {
            return Err(PoolError::NotLiquidatable);
        }
        let price = fresh_price(&env, &config)?;
        let collateral = CollateralVaultClient::new(&env, &config.vault).collateral_of(&owner);
        if is_healthy(
            collateral,
            position.debt,
            price,
            config.liquidation_threshold_bps,
        )
        .ok_or(PoolError::Overflow)?
        {
            return Err(PoolError::NotLiquidatable);
        }
        let debt = position.debt;
        let owed = collateral_seized(debt, price, config.liquidation_penalty_bps)
            .ok_or(PoolError::Overflow)?;
        let seized = owed.min(collateral);
        burn_debt(&env, &config, &liquidator, debt)?;
        position.debt = 0;
        position.last_accrual = 0;
        adjust_total(&env, -debt)?;
        save_position(&env, &owner, &position);
        CollateralVaultClient::new(&env, &config.vault).seize(&owner, &liquidator, &seized);
        let result = LiquidationResult {
            debt_repaid: debt,
            collateral_seized: seized,
            collateral_remaining: collateral - seized,
        };
        env.events().publish(
            (symbol_short!("liquidate"), owner, liquidator),
            (debt, seized),
        );
        Ok(result)
    }

    pub fn accrue(env: Env, owner: Address) -> Result<DebtPosition, PoolError> {
        let config = load_config(&env)?;
        let mut position = load_position(&env, &owner).ok_or(PoolError::NoPosition)?;
        accrue(&env, &config, &owner, &mut position)?;
        save_position(&env, &owner, &position);
        Ok(position)
    }

    pub fn position(env: Env, owner: Address) -> DebtPosition {
        load_position(&env, &owner).unwrap_or_default()
    }

    pub fn debt_of(env: Env, owner: Address) -> Result<i128, PoolError> {
        let config = load_config(&env)?;
        let position = load_position(&env, &owner).unwrap_or_default();
        current_debt(&env, &config, &position)
    }

    pub fn max_borrow(env: Env, owner: Address) -> Result<i128, PoolError> {
        let config = load_config(&env)?;
        let collateral = CollateralVaultClient::new(&env, &config.vault).collateral_of(&owner);
        let debt = current_debt(
            &env,
            &config,
            &load_position(&env, &owner).unwrap_or_default(),
        )?;
        let price = fresh_price(&env, &config)?;
        crate::max_borrow(collateral, price, debt, config.max_ltv_bps)
            .ok_or(PoolError::Overflow)
            .map(|remaining| remaining.max(0))
    }

    pub fn health_factor(env: Env, owner: Address) -> Result<i128, PoolError> {
        let config = load_config(&env)?;
        let collateral = CollateralVaultClient::new(&env, &config.vault).collateral_of(&owner);
        let debt = current_debt(
            &env,
            &config,
            &load_position(&env, &owner).unwrap_or_default(),
        )?;
        let price = fresh_price(&env, &config)?;
        health_factor_bps(collateral, debt, price).ok_or(PoolError::Overflow)
    }

    pub fn is_liquidatable(env: Env, owner: Address) -> Result<bool, PoolError> {
        let config = load_config(&env)?;
        let collateral = CollateralVaultClient::new(&env, &config.vault).collateral_of(&owner);
        let debt = current_debt(
            &env,
            &config,
            &load_position(&env, &owner).unwrap_or_default(),
        )?;
        if debt <= 0 {
            return Ok(false);
        }
        let price = fresh_price(&env, &config)?;
        Ok(
            !is_healthy(collateral, debt, price, config.liquidation_threshold_bps)
                .ok_or(PoolError::Overflow)?,
        )
    }

    pub fn required_collateral(env: Env, owner: Address) -> Result<i128, PoolError> {
        let config = load_config(&env)?;
        let debt = current_debt(
            &env,
            &config,
            &load_position(&env, &owner).unwrap_or_default(),
        )?;
        if debt <= 0 {
            return Ok(0);
        }
        let price = fresh_price(&env, &config)?;
        debt.checked_mul(crate::PRICE_SCALE)
            .ok_or(PoolError::Overflow)?
            .checked_div(price)
            .ok_or(PoolError::Overflow)
    }

    pub fn total_debt(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&PoolKey::TotalDebt)
            .unwrap_or(0)
    }

    pub fn config(env: Env) -> Result<PoolConfig, PoolError> {
        load_config(&env)
    }
}

fn load_config(env: &Env) -> Result<PoolConfig, PoolError> {
    env.storage()
        .instance()
        .get(&PoolKey::Config)
        .ok_or(PoolError::NotInitialized)
}

fn load_position(env: &Env, owner: &Address) -> Option<DebtPosition> {
    env.storage()
        .persistent()
        .get(&PoolKey::Position(owner.clone()))
}

fn save_position(env: &Env, owner: &Address, position: &DebtPosition) {
    let key = PoolKey::Position(owner.clone());
    if position.debt == 0 && position.last_accrual == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, position);
    }
}

fn adjust_total(env: &Env, delta: i128) -> Result<(), PoolError> {
    let current: i128 = env
        .storage()
        .instance()
        .get(&PoolKey::TotalDebt)
        .unwrap_or(0);
    let next = current.checked_add(delta).ok_or(PoolError::Overflow)?;
    env.storage().instance().set(&PoolKey::TotalDebt, &next);
    Ok(())
}

fn current_debt(
    env: &Env,
    config: &PoolConfig,
    position: &DebtPosition,
) -> Result<i128, PoolError> {
    let elapsed = env
        .ledger()
        .timestamp()
        .saturating_sub(position.last_accrual);
    let interest = accrued_interest(position.debt, config.interest_rate_bps, elapsed)
        .ok_or(PoolError::Overflow)?;
    position
        .debt
        .checked_add(interest)
        .ok_or(PoolError::Overflow)
}

fn accrue(
    env: &Env,
    config: &PoolConfig,
    owner: &Address,
    position: &mut DebtPosition,
) -> Result<(), PoolError> {
    let now = env.ledger().timestamp();
    if position.debt == 0 {
        position.last_accrual = 0;
        return Ok(());
    }
    let elapsed = now.saturating_sub(position.last_accrual);
    let interest = accrued_interest(position.debt, config.interest_rate_bps, elapsed)
        .ok_or(PoolError::Overflow)?;
    if interest > 0 {
        position.debt = position
            .debt
            .checked_add(interest)
            .ok_or(PoolError::Overflow)?;
        position.last_accrual = now;
        adjust_total(env, interest)?;
        env.events()
            .publish((symbol_short!("interest"), owner.clone()), interest);
    }
    Ok(())
}

fn fresh_price(env: &Env, config: &PoolConfig) -> Result<i128, PoolError> {
    let data = match LendingOracleClient::new(env, &config.oracle).try_price() {
        Ok(Ok(data)) => data,
        _ => return Err(PoolError::PriceUnavailable),
    };
    let age = env.ledger().timestamp().saturating_sub(data.timestamp);
    if age > config.max_price_age {
        return Err(PoolError::StalePrice);
    }
    Ok(data.price)
}

fn burn_debt(
    env: &Env,
    config: &PoolConfig,
    from: &Address,
    amount: i128,
) -> Result<(), PoolError> {
    match DebtTokenClient::new(env, &config.debt_token).try_burn(from, &amount) {
        Ok(Ok(())) => Ok(()),
        _ => Err(PoolError::InsufficientDebtToken),
    }
}
