#![no_std]
#![allow(deprecated)]
#![allow(clippy::too_many_arguments)]

use soroban_sdk::token::TokenClient;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Symbol,
};

/// 7 decimals precision for spot and TWAP price representations.
pub const PRICE_PRECISION: i128 = 10_000_000;

/// Denominator for basis points (100% = 10,000 bps).
pub const BPS_DENOMINATOR: i128 = 10_000;

/// Default minimum fee in basis points (0.10%).
pub const DEFAULT_MIN_FEE_BPS: u32 = 10;

/// Default base fee in basis points (0.30%).
pub const DEFAULT_BASE_FEE_BPS: u32 = 30;

/// Default maximum fee in basis points (5.00%).
pub const DEFAULT_MAX_FEE_BPS: u32 = 500;

/// Default multiplier scaling price deviation into fee basis points.
pub const DEFAULT_VOLATILITY_FACTOR: u32 = 2;

/// Default TWAP observation window in seconds (5 minutes).
pub const DEFAULT_TWAP_WINDOW: u64 = 300;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DynamicAmmError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidParameter = 3,
    ZeroAmount = 4,
    InsufficientLiquidity = 5,
    SlippageExceeded = 6,
    InsufficientShares = 7,
    InvalidToken = 8,
    ArithmeticError = 9,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoolConfig {
    pub admin: Address,
    pub token_a: Address,
    pub token_b: Address,
    pub min_fee_bps: u32,
    pub max_fee_bps: u32,
    pub base_fee_bps: u32,
    pub volatility_factor: u32,
    pub twap_window: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TwapState {
    pub price_cumulative: i128,
    pub last_timestamp: u64,
    pub last_spot_price: i128,
    pub twap_price: i128,
    pub window_start_timestamp: u64,
    pub window_start_price_cumulative: i128,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Reserves,
    TotalShares,
    Share(Address),
    Twap,
}

/// Computes the integer square root using the Babylonian method.
pub fn integer_sqrt(y: i128) -> i128 {
    if y <= 0 {
        return 0;
    }
    let mut z = y;
    let mut x = y / 2 + 1;
    while x < z {
        z = x;
        x = (y / x + x) / 2;
    }
    z
}

/// Calculates spot price of token A in terms of token B with 7 decimals precision:
/// `spot = (reserve_b * PRICE_PRECISION) / reserve_a`.
pub fn calculate_spot_price(reserve_a: i128, reserve_b: i128) -> Option<i128> {
    if reserve_a <= 0 || reserve_b <= 0 {
        return None;
    }
    reserve_b
        .checked_mul(PRICE_PRECISION)?
        .checked_div(reserve_a)
}

/// Calculates the dynamic fee in basis points based on price divergence from the TWAP.
///
/// Formula:
/// `deviation_bps = (|spot_price - twap_price| * 10_000) / twap_price`
/// `extra_fee = deviation_bps * volatility_factor`
/// `fee_bps = (base_fee_bps + extra_fee).clamp(min_fee_bps, max_fee_bps)`
pub fn calculate_dynamic_fee_bps(
    spot_price: i128,
    twap_price: i128,
    base_fee_bps: u32,
    min_fee_bps: u32,
    max_fee_bps: u32,
    volatility_factor: u32,
) -> u32 {
    if spot_price <= 0 || twap_price <= 0 {
        return base_fee_bps.clamp(min_fee_bps, max_fee_bps);
    }

    let diff = if spot_price >= twap_price {
        spot_price - twap_price
    } else {
        twap_price - spot_price
    };

    let deviation_bps = match diff.checked_mul(BPS_DENOMINATOR) {
        Some(product) => (product / twap_price) as u32,
        None => return max_fee_bps,
    };

    let extra_fee = deviation_bps.saturating_mul(volatility_factor);
    let raw_fee = base_fee_bps.saturating_add(extra_fee);
    raw_fee.clamp(min_fee_bps, max_fee_bps)
}

/// Calculates swap output and fee amount given constant product reserves and fee in bps.
///
/// Returns `Some((amount_out, fee_amount))` or `None` on overflow or insufficient liquidity.
pub fn calculate_amount_out(
    amount_in: i128,
    reserve_in: i128,
    reserve_out: i128,
    fee_bps: u32,
) -> Option<(i128, i128)> {
    if amount_in <= 0 || reserve_in <= 0 || reserve_out <= 0 {
        return None;
    }

    let fee_amount = amount_in
        .checked_mul(fee_bps as i128)?
        .checked_div(BPS_DENOMINATOR)?;
    let effective_in = amount_in.checked_sub(fee_amount)?;

    let numerator = reserve_out.checked_mul(effective_in)?;
    let denominator = reserve_in.checked_add(effective_in)?;
    let amount_out = numerator.checked_div(denominator)?;

    Some((amount_out, fee_amount))
}

#[contract]
pub struct DynamicAmmContract;

#[contractimpl]
impl DynamicAmmContract {
    /// Initializes the dynamic AMM pool with two tokens, fee bounds, and TWAP window.
    pub fn initialize(
        env: Env,
        admin: Address,
        token_a: Address,
        token_b: Address,
        min_fee_bps: u32,
        max_fee_bps: u32,
        base_fee_bps: u32,
        volatility_factor: u32,
        twap_window: u64,
    ) -> Result<(), DynamicAmmError> {
        admin.require_auth();

        if env.storage().instance().has(&DataKey::Config) {
            return Err(DynamicAmmError::AlreadyInitialized);
        }
        if token_a == token_b {
            return Err(DynamicAmmError::InvalidToken);
        }
        if min_fee_bps > base_fee_bps
            || base_fee_bps > max_fee_bps
            || max_fee_bps > BPS_DENOMINATOR as u32
            || twap_window == 0
        {
            return Err(DynamicAmmError::InvalidParameter);
        }

        let config = PoolConfig {
            admin,
            token_a,
            token_b,
            min_fee_bps,
            max_fee_bps,
            base_fee_bps,
            volatility_factor,
            twap_window,
        };

        let now = env.ledger().timestamp();
        let twap = TwapState {
            price_cumulative: 0,
            last_timestamp: now,
            last_spot_price: 0,
            twap_price: 0,
            window_start_timestamp: now,
            window_start_price_cumulative: 0,
        };

        env.storage().instance().set(&DataKey::Config, &config);
        env.storage()
            .instance()
            .set(&DataKey::Reserves, &(0i128, 0i128));
        env.storage().instance().set(&DataKey::TotalShares, &0i128);
        env.storage().instance().set(&DataKey::Twap, &twap);

        env.events().publish(
            (symbol_short!("init"), config.token_a.clone()),
            (config.token_b.clone(), config.base_fee_bps),
        );

        Ok(())
    }

    /// Deposits liquidity into the pool, minting LP shares.
    pub fn deposit(
        env: Env,
        sender: Address,
        amount_a: i128,
        amount_b: i128,
        min_shares: i128,
    ) -> Result<i128, DynamicAmmError> {
        sender.require_auth();

        let config: PoolConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)?;

        if amount_a <= 0 || amount_b <= 0 {
            return Err(DynamicAmmError::ZeroAmount);
        }

        Self::update_twap_internal(&env, &config)?;

        let (mut reserve_a, mut reserve_b): (i128, i128) = env
            .storage()
            .instance()
            .get(&DataKey::Reserves)
            .unwrap_or((0, 0));
        let total_shares: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalShares)
            .unwrap_or(0);

        let shares = if total_shares == 0 {
            let product = amount_a
                .checked_mul(amount_b)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            integer_sqrt(product)
        } else {
            let share_a = amount_a
                .checked_mul(total_shares)
                .ok_or(DynamicAmmError::ArithmeticError)?
                .checked_div(reserve_a)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            let share_b = amount_b
                .checked_mul(total_shares)
                .ok_or(DynamicAmmError::ArithmeticError)?
                .checked_div(reserve_b)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            share_a.min(share_b)
        };

        if shares < min_shares || shares <= 0 {
            return Err(DynamicAmmError::SlippageExceeded);
        }

        let contract_address = env.current_contract_address();
        TokenClient::new(&env, &config.token_a).transfer(&sender, &contract_address, &amount_a);
        TokenClient::new(&env, &config.token_b).transfer(&sender, &contract_address, &amount_b);

        reserve_a = reserve_a
            .checked_add(amount_a)
            .ok_or(DynamicAmmError::ArithmeticError)?;
        reserve_b = reserve_b
            .checked_add(amount_b)
            .ok_or(DynamicAmmError::ArithmeticError)?;
        let new_total_shares = total_shares
            .checked_add(shares)
            .ok_or(DynamicAmmError::ArithmeticError)?;

        let user_shares: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::Share(sender.clone()))
            .unwrap_or(0);
        let new_user_shares = user_shares
            .checked_add(shares)
            .ok_or(DynamicAmmError::ArithmeticError)?;

        env.storage()
            .instance()
            .set(&DataKey::Reserves, &(reserve_a, reserve_b));
        env.storage()
            .instance()
            .set(&DataKey::TotalShares, &new_total_shares);
        env.storage()
            .persistent()
            .set(&DataKey::Share(sender.clone()), &new_user_shares);

        Self::record_spot_price_internal(&env, reserve_a, reserve_b)?;

        env.events().publish(
            (symbol_short!("deposit"), sender),
            (amount_a, amount_b, shares),
        );

        Ok(shares)
    }

    /// Withdraws liquidity from the pool by burning LP shares.
    pub fn withdraw(
        env: Env,
        sender: Address,
        shares: i128,
        min_a: i128,
        min_b: i128,
    ) -> Result<(i128, i128), DynamicAmmError> {
        sender.require_auth();

        let config: PoolConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)?;

        if shares <= 0 {
            return Err(DynamicAmmError::ZeroAmount);
        }

        let user_shares: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::Share(sender.clone()))
            .unwrap_or(0);
        if user_shares < shares {
            return Err(DynamicAmmError::InsufficientShares);
        }

        Self::update_twap_internal(&env, &config)?;

        let (mut reserve_a, mut reserve_b): (i128, i128) = env
            .storage()
            .instance()
            .get(&DataKey::Reserves)
            .unwrap_or((0, 0));
        let total_shares: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalShares)
            .unwrap_or(0);

        if total_shares == 0 {
            return Err(DynamicAmmError::InsufficientLiquidity);
        }

        let amount_a = shares
            .checked_mul(reserve_a)
            .ok_or(DynamicAmmError::ArithmeticError)?
            .checked_div(total_shares)
            .ok_or(DynamicAmmError::ArithmeticError)?;
        let amount_b = shares
            .checked_mul(reserve_b)
            .ok_or(DynamicAmmError::ArithmeticError)?
            .checked_div(total_shares)
            .ok_or(DynamicAmmError::ArithmeticError)?;

        if amount_a < min_a || amount_b < min_b {
            return Err(DynamicAmmError::SlippageExceeded);
        }

        reserve_a = reserve_a
            .checked_sub(amount_a)
            .ok_or(DynamicAmmError::ArithmeticError)?;
        reserve_b = reserve_b
            .checked_sub(amount_b)
            .ok_or(DynamicAmmError::ArithmeticError)?;
        let new_total_shares = total_shares
            .checked_sub(shares)
            .ok_or(DynamicAmmError::ArithmeticError)?;
        let new_user_shares = user_shares
            .checked_sub(shares)
            .ok_or(DynamicAmmError::ArithmeticError)?;

        if new_user_shares == 0 {
            env.storage()
                .persistent()
                .remove(&DataKey::Share(sender.clone()));
        } else {
            env.storage()
                .persistent()
                .set(&DataKey::Share(sender.clone()), &new_user_shares);
        }

        env.storage()
            .instance()
            .set(&DataKey::Reserves, &(reserve_a, reserve_b));
        env.storage()
            .instance()
            .set(&DataKey::TotalShares, &new_total_shares);

        let contract_address = env.current_contract_address();
        TokenClient::new(&env, &config.token_a).transfer(&contract_address, &sender, &amount_a);
        TokenClient::new(&env, &config.token_b).transfer(&contract_address, &sender, &amount_b);

        Self::record_spot_price_internal(&env, reserve_a, reserve_b)?;

        env.events().publish(
            (symbol_short!("withdraw"), sender),
            (amount_a, amount_b, shares),
        );

        Ok((amount_a, amount_b))
    }

    /// Swaps tokens using volatility-adjusted dynamic fees.
    pub fn swap(
        env: Env,
        sender: Address,
        token_in: Address,
        amount_in: i128,
        min_amount_out: i128,
    ) -> Result<i128, DynamicAmmError> {
        sender.require_auth();

        let config: PoolConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)?;

        if amount_in <= 0 {
            return Err(DynamicAmmError::ZeroAmount);
        }

        Self::update_twap_internal(&env, &config)?;

        let fee_bps = Self::get_dynamic_fee_bps(env.clone())?;
        Self::execute_swap_internal(
            &env,
            &config,
            &sender,
            &token_in,
            amount_in,
            min_amount_out,
            fee_bps,
        )
    }

    /// Swaps tokens using a constant fee in basis points (for benchmarking against standard AMMs).
    pub fn swap_constant_fee(
        env: Env,
        sender: Address,
        token_in: Address,
        amount_in: i128,
        min_amount_out: i128,
        fixed_fee_bps: u32,
    ) -> Result<i128, DynamicAmmError> {
        sender.require_auth();

        let config: PoolConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)?;

        if amount_in <= 0 {
            return Err(DynamicAmmError::ZeroAmount);
        }

        Self::execute_swap_internal(
            &env,
            &config,
            &sender,
            &token_in,
            amount_in,
            min_amount_out,
            fixed_fee_bps,
        )
    }

    /// Synchronizes the TWAP price accumulator to the current block timestamp.
    pub fn sync_twap(env: Env) -> Result<(), DynamicAmmError> {
        let config: PoolConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)?;
        Self::update_twap_internal(&env, &config)
    }

    /// Returns current pool reserves `(reserve_a, reserve_b)`.
    pub fn get_reserves(env: Env) -> (i128, i128) {
        env.storage()
            .instance()
            .get(&DataKey::Reserves)
            .unwrap_or((0, 0))
    }

    /// Returns current instantaneous spot price scaled by `PRICE_PRECISION`.
    pub fn get_spot_price(env: Env) -> Result<i128, DynamicAmmError> {
        let (reserve_a, reserve_b) = Self::get_reserves(env);
        calculate_spot_price(reserve_a, reserve_b).ok_or(DynamicAmmError::InsufficientLiquidity)
    }

    /// Returns current Time-Weighted Average Price (TWAP).
    pub fn get_twap_price(env: Env) -> Result<i128, DynamicAmmError> {
        let twap: TwapState = env
            .storage()
            .instance()
            .get(&DataKey::Twap)
            .ok_or(DynamicAmmError::NotInitialized)?;

        if twap.twap_price > 0 {
            Ok(twap.twap_price)
        } else if twap.last_spot_price > 0 {
            Ok(twap.last_spot_price)
        } else {
            Self::get_spot_price(env)
        }
    }

    /// Returns the currently active dynamic fee in basis points.
    pub fn get_dynamic_fee_bps(env: Env) -> Result<u32, DynamicAmmError> {
        let config: PoolConfig = env
            .storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)?;

        let spot = match Self::get_spot_price(env.clone()) {
            Ok(price) => price,
            Err(_) => return Ok(config.base_fee_bps),
        };
        let twap = match Self::get_twap_price(env) {
            Ok(price) => price,
            Err(_) => spot,
        };

        Ok(calculate_dynamic_fee_bps(
            spot,
            twap,
            config.base_fee_bps,
            config.min_fee_bps,
            config.max_fee_bps,
            config.volatility_factor,
        ))
    }

    /// Returns total circulating LP shares.
    pub fn get_total_shares(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalShares)
            .unwrap_or(0)
    }

    /// Returns user's LP shares.
    pub fn get_shares(env: Env, user: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Share(user))
            .unwrap_or(0)
    }

    /// Returns pool configuration.
    pub fn get_config(env: Env) -> Result<PoolConfig, DynamicAmmError> {
        env.storage()
            .instance()
            .get(&DataKey::Config)
            .ok_or(DynamicAmmError::NotInitialized)
    }

    /// Quotes the output amount and applied fee for a given input.
    pub fn quote_swap(
        env: Env,
        token_in: Address,
        amount_in: i128,
    ) -> Result<(i128, u32), DynamicAmmError> {
        let config = Self::get_config(env.clone())?;
        let (reserve_a, reserve_b) = Self::get_reserves(env.clone());

        let (reserve_in, reserve_out) = if token_in == config.token_a {
            (reserve_a, reserve_b)
        } else if token_in == config.token_b {
            (reserve_b, reserve_a)
        } else {
            return Err(DynamicAmmError::InvalidToken);
        };

        let fee_bps = Self::get_dynamic_fee_bps(env)?;
        let (amount_out, _) = calculate_amount_out(amount_in, reserve_in, reserve_out, fee_bps)
            .ok_or(DynamicAmmError::InsufficientLiquidity)?;

        Ok((amount_out, fee_bps))
    }

    // --- Internal Helpers ---

    fn update_twap_internal(env: &Env, config: &PoolConfig) -> Result<(), DynamicAmmError> {
        let mut twap: TwapState = env
            .storage()
            .instance()
            .get(&DataKey::Twap)
            .ok_or(DynamicAmmError::NotInitialized)?;

        let now = env.ledger().timestamp();
        let time_elapsed = now.saturating_sub(twap.last_timestamp);

        if time_elapsed > 0 && twap.last_spot_price > 0 {
            let incremental_cumulative = twap
                .last_spot_price
                .checked_mul(time_elapsed as i128)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            twap.price_cumulative = twap
                .price_cumulative
                .checked_add(incremental_cumulative)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            twap.last_timestamp = now;

            let window_elapsed = now.saturating_sub(twap.window_start_timestamp);
            if window_elapsed >= config.twap_window {
                let cumulative_diff = twap
                    .price_cumulative
                    .checked_sub(twap.window_start_price_cumulative)
                    .ok_or(DynamicAmmError::ArithmeticError)?;
                twap.twap_price = cumulative_diff
                    .checked_div(window_elapsed as i128)
                    .ok_or(DynamicAmmError::ArithmeticError)?;
                twap.window_start_timestamp = now;
                twap.window_start_price_cumulative = twap.price_cumulative;

                env.events().publish(
                    (symbol_short!("twap"), Symbol::new(env, "window")),
                    (twap.twap_price, now),
                );
            }

            env.storage().instance().set(&DataKey::Twap, &twap);
        }

        Ok(())
    }

    fn record_spot_price_internal(
        env: &Env,
        reserve_a: i128,
        reserve_b: i128,
    ) -> Result<(), DynamicAmmError> {
        if let Some(spot) = calculate_spot_price(reserve_a, reserve_b) {
            let mut twap: TwapState = env
                .storage()
                .instance()
                .get(&DataKey::Twap)
                .ok_or(DynamicAmmError::NotInitialized)?;
            twap.last_spot_price = spot;
            if twap.twap_price == 0 {
                twap.twap_price = spot;
            }
            env.storage().instance().set(&DataKey::Twap, &twap);
        }
        Ok(())
    }

    fn execute_swap_internal(
        env: &Env,
        config: &PoolConfig,
        sender: &Address,
        token_in: &Address,
        amount_in: i128,
        min_amount_out: i128,
        fee_bps: u32,
    ) -> Result<i128, DynamicAmmError> {
        let (mut reserve_a, mut reserve_b): (i128, i128) = env
            .storage()
            .instance()
            .get(&DataKey::Reserves)
            .unwrap_or((0, 0));

        let is_a_to_b = if *token_in == config.token_a {
            true
        } else if *token_in == config.token_b {
            false
        } else {
            return Err(DynamicAmmError::InvalidToken);
        };

        let (reserve_in, reserve_out) = if is_a_to_b {
            (reserve_a, reserve_b)
        } else {
            (reserve_b, reserve_a)
        };

        let (amount_out, fee_amount) =
            calculate_amount_out(amount_in, reserve_in, reserve_out, fee_bps)
                .ok_or(DynamicAmmError::InsufficientLiquidity)?;

        if amount_out < min_amount_out {
            return Err(DynamicAmmError::SlippageExceeded);
        }

        if is_a_to_b {
            reserve_a = reserve_a
                .checked_add(amount_in)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            reserve_b = reserve_b
                .checked_sub(amount_out)
                .ok_or(DynamicAmmError::ArithmeticError)?;
        } else {
            reserve_b = reserve_b
                .checked_add(amount_in)
                .ok_or(DynamicAmmError::ArithmeticError)?;
            reserve_a = reserve_a
                .checked_sub(amount_out)
                .ok_or(DynamicAmmError::ArithmeticError)?;
        }

        env.storage()
            .instance()
            .set(&DataKey::Reserves, &(reserve_a, reserve_b));

        let contract_address = env.current_contract_address();
        let token_out = if is_a_to_b {
            &config.token_b
        } else {
            &config.token_a
        };

        TokenClient::new(env, token_in).transfer(sender, &contract_address, &amount_in);
        TokenClient::new(env, token_out).transfer(&contract_address, sender, &amount_out);

        Self::record_spot_price_internal(env, reserve_a, reserve_b)?;

        env.events().publish(
            (symbol_short!("swap"), sender.clone(), token_in.clone()),
            (amount_in, amount_out, fee_bps, fee_amount),
        );

        Ok(amount_out)
    }
}
