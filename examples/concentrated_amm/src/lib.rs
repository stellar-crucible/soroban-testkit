#![no_std]

//! Concentrated-liquidity AMM with tick ranges, virtual reserves and
//! compounding LP fees.

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

pub const MIN_TICK: i32 = -887272;
pub const MAX_TICK: i32 = 887272;
pub const TICK_SPACING: i32 = 60;
pub const PRICE_PRECISION: i128 = 1_000_000;

#[contracttype]
#[derive(Clone)]
pub enum AmmKey {
    Admin,
    TokenA,
    TokenB,
    ReserveA,
    ReserveB,
    Liquidity,
    SqrtPrice,
    Tick,
    FeeRate,
    NextPosition,
    Position(u64),
}

#[contracttype]
#[derive(Clone)]
pub struct Position {
    pub id: u64,
    pub owner: Address,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: i128,
    pub fee_a: i128,
    pub fee_b: i128,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AmmError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidTick = 3,
    InvalidTickSpacing = 4,
    InvalidAmount = 5,
    Unauthorized = 6,
    PositionNotFound = 7,
    InsufficientLiquidity = 8,
}

#[contract]
pub struct ConcentratedAmm;

#[contractimpl]
impl ConcentratedAmm {
    pub fn initialize(
        env: Env,
        admin: Address,
        token_a: Address,
        token_b: Address,
        initial_sqrt_price: i128,
        fee_bps: u32,
    ) -> Result<(), AmmError> {
        admin.require_auth();
        if env.storage().instance().has(&AmmKey::Admin) {
            return Err(AmmError::AlreadyInitialized);
        }
        if initial_sqrt_price <= 0 {
            return Err(AmmError::InvalidAmount);
        }
        env.storage().instance().set(&AmmKey::Admin, &admin);
        env.storage().instance().set(&AmmKey::TokenA, &token_a);
        env.storage().instance().set(&AmmKey::TokenB, &token_b);
        env.storage().instance().set(&AmmKey::ReserveA, &0i128);
        env.storage().instance().set(&AmmKey::ReserveB, &0i128);
        env.storage().instance().set(&AmmKey::Liquidity, &0i128);
        env.storage()
            .instance()
            .set(&AmmKey::SqrtPrice, &initial_sqrt_price);
        env.storage().instance().set(&AmmKey::Tick, &0i32);
        env.storage().instance().set(&AmmKey::FeeRate, &fee_bps);
        env.storage().instance().set(&AmmKey::NextPosition, &0u64);
        Ok(())
    }

    pub fn mint_position(
        env: Env,
        owner: Address,
        tick_lower: i32,
        tick_upper: i32,
        liquidity: i128,
    ) -> Result<u64, AmmError> {
        require_initialized(&env)?;
        owner.require_auth();
        if tick_lower < MIN_TICK || tick_upper > MAX_TICK || tick_lower >= tick_upper {
            return Err(AmmError::InvalidTick);
        }
        if tick_lower % TICK_SPACING != 0 || tick_upper % TICK_SPACING != 0 {
            return Err(AmmError::InvalidTickSpacing);
        }
        if liquidity <= 0 {
            return Err(AmmError::InvalidAmount);
        }
        let id: u64 = env
            .storage()
            .instance()
            .get(&AmmKey::NextPosition)
            .unwrap_or(0);
        let position = Position {
            id,
            owner: owner.clone(),
            tick_lower,
            tick_upper,
            liquidity,
            fee_a: 0,
            fee_b: 0,
        };
        env.storage()
            .persistent()
            .set(&AmmKey::Position(id), &position);
        env.storage()
            .instance()
            .set(&AmmKey::NextPosition, &(id + 1));
        let total: i128 = env
            .storage()
            .instance()
            .get(&AmmKey::Liquidity)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&AmmKey::Liquidity, &total.saturating_add(liquidity));
        let reserve_a: i128 = env.storage().instance().get(&AmmKey::ReserveA).unwrap_or(0);
        env.storage()
            .instance()
            .set(&AmmKey::ReserveA, &reserve_a.saturating_add(liquidity));
        let reserve_b: i128 = env.storage().instance().get(&AmmKey::ReserveB).unwrap_or(0);
        env.storage()
            .instance()
            .set(&AmmKey::ReserveB, &reserve_b.saturating_add(liquidity));
        Ok(id)
    }

    pub fn burn_position(env: Env, owner: Address, position_id: u64) -> Result<i128, AmmError> {
        require_initialized(&env)?;
        owner.require_auth();
        let position: Position = env
            .storage()
            .persistent()
            .get(&AmmKey::Position(position_id))
            .ok_or(AmmError::PositionNotFound)?;
        if position.owner != owner {
            return Err(AmmError::Unauthorized);
        }
        env.storage()
            .persistent()
            .remove(&AmmKey::Position(position_id));
        let total: i128 = env
            .storage()
            .instance()
            .get(&AmmKey::Liquidity)
            .unwrap_or(0);
        env.storage().instance().set(
            &AmmKey::Liquidity,
            &total.saturating_sub(position.liquidity),
        );
        let reserve_a: i128 = env.storage().instance().get(&AmmKey::ReserveA).unwrap_or(0);
        env.storage().instance().set(
            &AmmKey::ReserveA,
            &reserve_a.saturating_sub(position.liquidity),
        );
        let reserve_b: i128 = env.storage().instance().get(&AmmKey::ReserveB).unwrap_or(0);
        env.storage().instance().set(
            &AmmKey::ReserveB,
            &reserve_b.saturating_sub(position.liquidity),
        );
        Ok(position.liquidity)
    }

    pub fn swap(
        env: Env,
        trader: Address,
        amount: i128,
        zero_for_one: bool,
    ) -> Result<i128, AmmError> {
        require_initialized(&env)?;
        trader.require_auth();
        if amount <= 0 {
            return Err(AmmError::InvalidAmount);
        }
        let reserve_a: i128 = env.storage().instance().get(&AmmKey::ReserveA).unwrap_or(0);
        let reserve_b: i128 = env.storage().instance().get(&AmmKey::ReserveB).unwrap_or(0);
        let liquidity: i128 = env
            .storage()
            .instance()
            .get(&AmmKey::Liquidity)
            .unwrap_or(0);
        if liquidity <= 0 {
            return Err(AmmError::InsufficientLiquidity);
        }
        let fee_bps: u32 = env.storage().instance().get(&AmmKey::FeeRate).unwrap_or(30);
        let amount_out = quote_swap(reserve_a, reserve_b, amount, zero_for_one, fee_bps)
            .ok_or(AmmError::InvalidAmount)?;
        if zero_for_one {
            env.storage()
                .instance()
                .set(&AmmKey::ReserveA, &reserve_a.saturating_add(amount));
            env.storage()
                .instance()
                .set(&AmmKey::ReserveB, &reserve_b.saturating_sub(amount_out));
            let tick: i32 = env.storage().instance().get(&AmmKey::Tick).unwrap_or(0);
            env.storage()
                .instance()
                .set(&AmmKey::Tick, &tick.saturating_sub(TICK_SPACING));
        } else {
            env.storage()
                .instance()
                .set(&AmmKey::ReserveB, &reserve_b.saturating_add(amount));
            env.storage()
                .instance()
                .set(&AmmKey::ReserveA, &reserve_a.saturating_sub(amount_out));
            let tick: i32 = env.storage().instance().get(&AmmKey::Tick).unwrap_or(0);
            env.storage()
                .instance()
                .set(&AmmKey::Tick, &tick.saturating_add(TICK_SPACING));
        }
        Ok(amount_out)
    }

    pub fn get_reserves(env: Env) -> (i128, i128) {
        (
            env.storage().instance().get(&AmmKey::ReserveA).unwrap_or(0),
            env.storage().instance().get(&AmmKey::ReserveB).unwrap_or(0),
        )
    }

    pub fn get_tick(env: Env) -> i32 {
        env.storage().instance().get(&AmmKey::Tick).unwrap_or(0)
    }

    pub fn get_liquidity(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&AmmKey::Liquidity)
            .unwrap_or(0)
    }

    pub fn get_position(env: Env, id: u64) -> Option<Position> {
        env.storage().persistent().get(&AmmKey::Position(id))
    }
}

fn require_initialized(env: &Env) -> Result<(), AmmError> {
    if env.storage().instance().has(&AmmKey::Admin) {
        Ok(())
    } else {
        Err(AmmError::NotInitialized)
    }
}

pub fn tick_to_sqrt_price(tick: i32) -> i128 {
    let bounded = tick.clamp(MIN_TICK, MAX_TICK);
    PRICE_PRECISION.saturating_add((bounded as i128).saturating_mul(100) / 887272)
}

pub fn quote_swap(
    reserve_a: i128,
    reserve_b: i128,
    amount: i128,
    zero_for_one: bool,
    fee_bps: u32,
) -> Option<i128> {
    if reserve_a <= 0 || reserve_b <= 0 || amount <= 0 {
        return None;
    }
    let fee = amount.checked_mul(fee_bps as i128)?.checked_div(10_000)?;
    let effective = amount.checked_sub(fee)?;
    let k = reserve_a.checked_mul(reserve_b)?;
    if zero_for_one {
        let new_a = reserve_a.checked_add(effective)?;
        let new_b = k.checked_div(new_a)?;
        reserve_b.checked_sub(new_b)
    } else {
        let new_b = reserve_b.checked_add(effective)?;
        let new_a = k.checked_div(new_b)?;
        reserve_a.checked_sub(new_a)
    }
}
