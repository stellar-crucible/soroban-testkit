#![no_std]

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, token, Address, Env, Symbol, Vec,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum PortfolioError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidInput = 3,
    Unauthorized = 4,
    InsufficientBalance = 5,
    SlippageExceeded = 6,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetConfig {
    pub token: Address,
    pub target_bps: u32,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Assets,
    TotalShares,
    ShareBalance(Address),
    OraclePrice(Address),
}

fn read_assets(env: &Env) -> Vec<AssetConfig> {
    env.storage()
        .instance()
        .get(&DataKey::Assets)
        .unwrap_or_else(|| Vec::new(env))
}

fn read_total_shares(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::TotalShares)
        .unwrap_or(0)
}

fn write_total_shares(env: &Env, total: i128) {
    env.storage().instance().set(&DataKey::TotalShares, &total);
}

fn read_share_balance(env: &Env, user: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::ShareBalance(user.clone()))
        .unwrap_or(0)
}

fn write_share_balance(env: &Env, user: &Address, amount: i128) {
    let key = DataKey::ShareBalance(user.clone());
    if amount <= 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &amount);
    }
}

fn require_initialized(env: &Env) -> Result<(), PortfolioError> {
    if !env.storage().instance().has(&DataKey::Admin) {
        return Err(PortfolioError::NotInitialized);
    }
    Ok(())
}

fn asset_value(amount: i128, price: i128) -> Result<i128, PortfolioError> {
    amount
        .checked_mul(price)
        .ok_or(PortfolioError::InvalidInput)
}

pub fn portfolio_total_value(env: &Env) -> Result<i128, PortfolioError> {
    let assets = read_assets(env);
    let mut total = 0i128;
    for cfg in assets.iter() {
        let bal = token::Client::new(env, &cfg.token).balance(&env.current_contract_address());
        let price: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::OraclePrice(cfg.token.clone()))
            .ok_or(PortfolioError::InvalidInput)?;
        total = total
            .checked_add(asset_value(bal, price)?)
            .ok_or(PortfolioError::InvalidInput)?;
    }
    Ok(total)
}

pub fn target_value_for_asset(total_value: i128, target_bps: u32) -> Result<i128, PortfolioError> {
    total_value
        .checked_mul(target_bps as i128)
        .and_then(|v| v.checked_div(10_000))
        .ok_or(PortfolioError::InvalidInput)
}

#[contract]
pub struct PortfolioVault;

#[contractimpl]
impl PortfolioVault {
    pub fn initialize(
        env: Env,
        admin: Address,
        assets: Vec<AssetConfig>,
    ) -> Result<(), PortfolioError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(PortfolioError::AlreadyInitialized);
        }
        if assets.len() == 0 {
            return Err(PortfolioError::InvalidInput);
        }
        let mut total_bps = 0u32;
        for cfg in assets.iter() {
            total_bps = total_bps
                .checked_add(cfg.target_bps)
                .ok_or(PortfolioError::InvalidInput)?;
        }
        if total_bps != 10_000 {
            return Err(PortfolioError::InvalidInput);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Assets, &assets);
        write_total_shares(&env, 0);
        Ok(())
    }

    pub fn set_price(
        env: Env,
        caller: Address,
        asset: Address,
        price: i128,
    ) -> Result<(), PortfolioError> {
        require_initialized(&env)?;
        caller.require_auth();
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        if caller != admin {
            return Err(PortfolioError::Unauthorized);
        }
        if price <= 0 {
            return Err(PortfolioError::InvalidInput);
        }
        env.storage()
            .persistent()
            .set(&DataKey::OraclePrice(asset), &price);
        Ok(())
    }

    pub fn deposit(
        env: Env,
        user: Address,
        asset: Address,
        amount: i128,
    ) -> Result<i128, PortfolioError> {
        require_initialized(&env)?;
        user.require_auth();
        if amount <= 0 {
            return Err(PortfolioError::InvalidInput);
        }
        let assets = read_assets(&env);
        let mut found = false;
        let mut asset_price = 0i128;
        for cfg in assets.iter() {
            if cfg.token == asset {
                found = true;
                asset_price = env
                    .storage()
                    .persistent()
                    .get(&DataKey::OraclePrice(asset.clone()))
                    .ok_or(PortfolioError::InvalidInput)?;
            }
        }
        if !found {
            return Err(PortfolioError::InvalidInput);
        }

        let total_value = portfolio_total_value(&env)?;
        let deposit_value = asset_value(amount, asset_price)?;
        let total_shares = read_total_shares(&env);
        let minted = if total_shares == 0 || total_value == 0 {
            deposit_value
        } else {
            deposit_value
                .checked_mul(total_shares)
                .and_then(|v| v.checked_div(total_value))
                .ok_or(PortfolioError::InvalidInput)?
        };
        if minted <= 0 {
            return Err(PortfolioError::InvalidInput);
        }

        token::Client::new(&env, &asset).transfer(&user, &env.current_contract_address(), &amount);
        write_total_shares(
            &env,
            total_shares
                .checked_add(minted)
                .ok_or(PortfolioError::InvalidInput)?,
        );
        let bal = read_share_balance(&env, &user);
        write_share_balance(
            &env,
            &user,
            bal.checked_add(minted)
                .ok_or(PortfolioError::InvalidInput)?,
        );
        Ok(minted)
    }

    pub fn withdraw(
        env: Env,
        user: Address,
        share_amount: i128,
    ) -> Result<Vec<(Address, i128)>, PortfolioError> {
        require_initialized(&env)?;
        user.require_auth();
        if share_amount <= 0 {
            return Err(PortfolioError::InvalidInput);
        }
        let user_shares = read_share_balance(&env, &user);
        if share_amount > user_shares {
            return Err(PortfolioError::InsufficientBalance);
        }
        let total_shares = read_total_shares(&env);
        if total_shares <= 0 {
            return Err(PortfolioError::InvalidInput);
        }

        let assets = read_assets(&env);
        let mut payouts: Vec<(Address, i128)> = Vec::new(&env);
        for cfg in assets.iter() {
            let bal = token::Client::new(&env, &cfg.token).balance(&env.current_contract_address());
            let payout = bal
                .checked_mul(share_amount)
                .and_then(|v| v.checked_div(total_shares))
                .ok_or(PortfolioError::InvalidInput)?;
            if payout > 0 {
                token::Client::new(&env, &cfg.token).transfer(
                    &env.current_contract_address(),
                    &user,
                    &payout,
                );
            }
            payouts.push_back((cfg.token.clone(), payout));
        }

        write_share_balance(&env, &user, user_shares - share_amount);
        write_total_shares(&env, total_shares - share_amount);
        Ok(payouts)
    }

    pub fn rebalance(
        env: Env,
        caller: Address,
        sell_asset: Address,
        buy_asset: Address,
        sell_amount: i128,
        min_buy_amount: i128,
    ) -> Result<i128, PortfolioError> {
        require_initialized(&env)?;
        caller.require_auth();
        if sell_amount <= 0 || min_buy_amount < 0 || sell_asset == buy_asset {
            return Err(PortfolioError::InvalidInput);
        }
        let sell_price: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::OraclePrice(sell_asset.clone()))
            .ok_or(PortfolioError::InvalidInput)?;
        let buy_price: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::OraclePrice(buy_asset.clone()))
            .ok_or(PortfolioError::InvalidInput)?;
        if sell_price <= 0 || buy_price <= 0 {
            return Err(PortfolioError::InvalidInput);
        }

        let sell_value = asset_value(sell_amount, sell_price)?;
        let buy_amount = sell_value
            .checked_div(buy_price)
            .ok_or(PortfolioError::InvalidInput)?;
        if buy_amount < min_buy_amount {
            return Err(PortfolioError::SlippageExceeded);
        }

        let vault = env.current_contract_address();
        let sell_client = token::Client::new(&env, &sell_asset);
        let buy_client = token::Client::new(&env, &buy_asset);
        let sell_bal = sell_client.balance(&vault);
        let buy_bal = buy_client.balance(&caller);
        if sell_bal < sell_amount || buy_bal < buy_amount {
            return Err(PortfolioError::InsufficientBalance);
        }

        // Simplified swap execution: vault sells overweight asset to caller at oracle price.
        sell_client.transfer(&vault, &caller, &sell_amount);
        buy_client.transfer(&caller, &vault, &buy_amount);
        Ok(buy_amount)
    }

    pub fn total_shares(env: Env) -> i128 {
        read_total_shares(&env)
    }

    pub fn share_balance(env: Env, user: Address) -> i128 {
        read_share_balance(&env, &user)
    }

    pub fn assets(env: Env) -> Vec<AssetConfig> {
        read_assets(&env)
    }

    pub fn price(env: Env, asset: Address) -> Option<i128> {
        env.storage().persistent().get(&DataKey::OraclePrice(asset))
    }

    pub fn total_value(env: Env) -> Result<i128, PortfolioError> {
        require_initialized(&env)?;
        portfolio_total_value(&env)
    }

    pub fn asset_weight_bps(env: Env, asset: Address) -> Result<u32, PortfolioError> {
        require_initialized(&env)?;
        let total = portfolio_total_value(&env)?;
        if total <= 0 {
            return Ok(0);
        }
        let assets = read_assets(&env);
        for cfg in assets.iter() {
            if cfg.token == asset {
                let bal =
                    token::Client::new(&env, &cfg.token).balance(&env.current_contract_address());
                let price: i128 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::OraclePrice(asset))
                    .ok_or(PortfolioError::InvalidInput)?;
                let value = asset_value(bal, price)?;
                let bps = value
                    .checked_mul(10_000)
                    .and_then(|v| v.checked_div(total))
                    .ok_or(PortfolioError::InvalidInput)? as u32;
                return Ok(bps);
            }
        }
        Err(PortfolioError::InvalidInput)
    }

    pub fn is_within_tolerance(
        env: Env,
        asset: Address,
        tolerance_bps: u32,
    ) -> Result<bool, PortfolioError> {
        require_initialized(&env)?;
        let assets = read_assets(&env);
        let mut target = 0u32;
        for cfg in assets.iter() {
            if cfg.token == asset {
                target = cfg.target_bps;
            }
        }
        let actual = Self::asset_weight_bps(env.clone(), asset)?;
        let diff = actual.abs_diff(target);
        Ok(diff <= tolerance_bps)
    }

    pub fn admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn contract_id(env: Env) -> Address {
        env.current_contract_address()
    }

    pub fn version(_env: Env) -> Symbol {
        Symbol::new(&_env, "v1")
    }
}
