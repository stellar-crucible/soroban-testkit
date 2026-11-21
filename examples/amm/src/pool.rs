use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

use crate::token::AmmTokenClient;
use crate::{optimal_deposit, quote_amount_out, shares_for_deposit, BPS, MINIMUM_LIQUIDITY};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AmmError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAmount = 3,
    SlippageExceeded = 4,
    InsufficientLiquidity = 5,
    InvalidFee = 6,
    InvariantViolated = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoolKey {
    TokenA,
    TokenB,
    ShareToken,
    FeeBps,
    ReserveA,
    ReserveB,
    TotalShares,
}

/// A two-token constant-product pool.
///
/// Reserves and share supply sit in instance storage, next to the pool's
/// configuration, because every operation reads all of them.
#[contract]
pub struct ConstantProductPool;

#[contractimpl]
impl ConstantProductPool {
    /// Configure the pool. `share_token` must be an [`crate::AmmToken`] whose
    /// admin is this pool, so the pool can mint LP shares.
    pub fn initialize(
        env: Env,
        token_a: Address,
        token_b: Address,
        share_token: Address,
        fee_bps: u32,
    ) -> Result<(), AmmError> {
        let instance = env.storage().instance();
        if instance.has(&PoolKey::TokenA) {
            return Err(AmmError::AlreadyInitialized);
        }
        if fee_bps as i128 >= BPS {
            return Err(AmmError::InvalidFee);
        }
        instance.set(&PoolKey::TokenA, &token_a);
        instance.set(&PoolKey::TokenB, &token_b);
        instance.set(&PoolKey::ShareToken, &share_token);
        instance.set(&PoolKey::FeeBps, &fee_bps);
        instance.set(&PoolKey::ReserveA, &0i128);
        instance.set(&PoolKey::ReserveB, &0i128);
        instance.set(&PoolKey::TotalShares, &0i128);
        Ok(())
    }

    /// Add liquidity, sending at most `desired_*` and at least `min_*` of
    /// each token, and return the LP shares minted to `to`.
    pub fn deposit(
        env: Env,
        to: Address,
        desired_a: i128,
        desired_b: i128,
        min_a: i128,
        min_b: i128,
    ) -> Result<i128, AmmError> {
        to.require_auth();
        if desired_a <= 0 || desired_b <= 0 {
            return Err(AmmError::InvalidAmount);
        }
        let (reserve_a, reserve_b) = Self::get_reserves(env.clone())?;
        let total = Self::total_shares(env.clone());

        let (amount_a, amount_b) = optimal_deposit(reserve_a, reserve_b, desired_a, desired_b)
            .ok_or(AmmError::InvalidAmount)?;
        if amount_a < min_a || amount_b < min_b {
            return Err(AmmError::SlippageExceeded);
        }
        let mut minted = shares_for_deposit(reserve_a, reserve_b, total, amount_a, amount_b)
            .ok_or(AmmError::InvalidAmount)?;
        let mut new_total = total + minted;
        if total == 0 {
            if minted <= MINIMUM_LIQUIDITY {
                return Err(AmmError::InsufficientLiquidity);
            }
            // Locked forever: counted in the supply, owned by nobody.
            minted -= MINIMUM_LIQUIDITY;
            new_total = minted + MINIMUM_LIQUIDITY;
        }
        if minted <= 0 {
            return Err(AmmError::InsufficientLiquidity);
        }

        let pool = env.current_contract_address();
        Self::token_a(&env).transfer(&to, &pool, &amount_a);
        Self::token_b(&env).transfer(&to, &pool, &amount_b);
        Self::share_token(&env).mint(&to, &minted);

        Self::put_reserves(&env, reserve_a + amount_a, reserve_b + amount_b);
        env.storage()
            .instance()
            .set(&PoolKey::TotalShares, &new_total);
        env.events()
            .publish((symbol_short!("deposit"), to), (amount_a, amount_b, minted));
        Ok(minted)
    }

    /// Swap `amount_in` of one token for the other. `buy_a` names the token
    /// received; the swap fails unless at least `min_out` comes back.
    pub fn swap(
        env: Env,
        to: Address,
        buy_a: bool,
        amount_in: i128,
        min_out: i128,
    ) -> Result<i128, AmmError> {
        to.require_auth();
        if amount_in <= 0 {
            return Err(AmmError::InvalidAmount);
        }
        let (reserve_a, reserve_b) = Self::get_reserves(env.clone())?;
        if reserve_a == 0 || reserve_b == 0 {
            return Err(AmmError::InsufficientLiquidity);
        }
        let fee = Self::fee_bps(env.clone())?;
        let (reserve_in, reserve_out) = if buy_a {
            (reserve_b, reserve_a)
        } else {
            (reserve_a, reserve_b)
        };
        let out = quote_amount_out(reserve_in, reserve_out, amount_in, fee)
            .ok_or(AmmError::InvalidAmount)?;
        if out <= 0 || out >= reserve_out {
            return Err(AmmError::InsufficientLiquidity);
        }
        if out < min_out {
            return Err(AmmError::SlippageExceeded);
        }

        let new_in = reserve_in + amount_in;
        let new_out = reserve_out - out;
        let k_before = reserve_in.checked_mul(reserve_out);
        let k_after = new_in.checked_mul(new_out);
        match (k_before, k_after) {
            (Some(before), Some(after)) if after >= before => {}
            _ => return Err(AmmError::InvariantViolated),
        }

        let pool = env.current_contract_address();
        let (sell, buy) = if buy_a {
            (Self::token_b(&env), Self::token_a(&env))
        } else {
            (Self::token_a(&env), Self::token_b(&env))
        };
        sell.transfer(&to, &pool, &amount_in);
        buy.transfer(&pool, &to, &out);

        if buy_a {
            Self::put_reserves(&env, new_out, new_in);
        } else {
            Self::put_reserves(&env, new_in, new_out);
        }
        env.events()
            .publish((symbol_short!("swap"), to), (amount_in, out, buy_a));
        Ok(out)
    }

    /// Burn `shares` and return the pro-rata reserves to `to`.
    pub fn withdraw(
        env: Env,
        to: Address,
        shares: i128,
        min_a: i128,
        min_b: i128,
    ) -> Result<(i128, i128), AmmError> {
        to.require_auth();
        if shares <= 0 {
            return Err(AmmError::InvalidAmount);
        }
        let (reserve_a, reserve_b) = Self::get_reserves(env.clone())?;
        let total = Self::total_shares(env.clone());
        if shares > total - MINIMUM_LIQUIDITY {
            return Err(AmmError::InsufficientLiquidity);
        }
        let amount_a = shares * reserve_a / total;
        let amount_b = shares * reserve_b / total;
        if amount_a < min_a || amount_b < min_b {
            return Err(AmmError::SlippageExceeded);
        }

        let pool = env.current_contract_address();
        Self::share_token(&env).burn(&to, &shares);
        Self::token_a(&env).transfer(&pool, &to, &amount_a);
        Self::token_b(&env).transfer(&pool, &to, &amount_b);

        Self::put_reserves(&env, reserve_a - amount_a, reserve_b - amount_b);
        env.storage()
            .instance()
            .set(&PoolKey::TotalShares, &(total - shares));
        env.events().publish(
            (symbol_short!("withdraw"), to),
            (amount_a, amount_b, shares),
        );
        Ok((amount_a, amount_b))
    }

    /// What `swap(_, buy_a, amount_in, _)` would return right now.
    pub fn quote(env: Env, buy_a: bool, amount_in: i128) -> Result<i128, AmmError> {
        let (reserve_a, reserve_b) = Self::get_reserves(env.clone())?;
        let fee = Self::fee_bps(env)?;
        let (reserve_in, reserve_out) = if buy_a {
            (reserve_b, reserve_a)
        } else {
            (reserve_a, reserve_b)
        };
        quote_amount_out(reserve_in, reserve_out, amount_in, fee).ok_or(AmmError::InvalidAmount)
    }

    pub fn get_reserves(env: Env) -> Result<(i128, i128), AmmError> {
        let instance = env.storage().instance();
        match (
            instance.get(&PoolKey::ReserveA),
            instance.get(&PoolKey::ReserveB),
        ) {
            (Some(a), Some(b)) => Ok((a, b)),
            _ => Err(AmmError::NotInitialized),
        }
    }

    pub fn total_shares(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&PoolKey::TotalShares)
            .unwrap_or(0)
    }

    pub fn fee_bps(env: Env) -> Result<u32, AmmError> {
        env.storage()
            .instance()
            .get(&PoolKey::FeeBps)
            .ok_or(AmmError::NotInitialized)
    }

    pub fn share_token_id(env: Env) -> Result<Address, AmmError> {
        env.storage()
            .instance()
            .get(&PoolKey::ShareToken)
            .ok_or(AmmError::NotInitialized)
    }
}

impl ConstantProductPool {
    fn token(env: &Env, key: PoolKey) -> AmmTokenClient<'_> {
        let id: Address = env.storage().instance().get(&key).unwrap();
        AmmTokenClient::new(env, &id)
    }

    fn token_a(env: &Env) -> AmmTokenClient<'_> {
        Self::token(env, PoolKey::TokenA)
    }

    fn token_b(env: &Env) -> AmmTokenClient<'_> {
        Self::token(env, PoolKey::TokenB)
    }

    fn share_token(env: &Env) -> AmmTokenClient<'_> {
        Self::token(env, PoolKey::ShareToken)
    }

    fn put_reserves(env: &Env, reserve_a: i128, reserve_b: i128) {
        env.storage().instance().set(&PoolKey::ReserveA, &reserve_a);
        env.storage().instance().set(&PoolKey::ReserveB, &reserve_b);
    }
}
