use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env,
};

use crate::pool::BorrowingPoolClient;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum VaultError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidAmount = 3,
    InsufficientCollateral = 4,
    ReservedCollateral = 5,
    Unauthorized = 6,
    Overflow = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VaultConfig {
    pub pool: Address,
    pub collateral_token: Address,
}

#[contracttype]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VaultPosition {
    pub collateral: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum VaultKey {
    Config,
    TotalCollateral,
    Position(Address),
}

#[contract]
pub struct CollateralVault;

#[contractimpl]
impl CollateralVault {
    pub fn initialize(env: Env, config: VaultConfig) -> Result<(), VaultError> {
        if env.storage().instance().has(&VaultKey::Config) {
            return Err(VaultError::AlreadyInitialized);
        }
        env.storage().instance().set(&VaultKey::Config, &config);
        env.storage()
            .instance()
            .set(&VaultKey::TotalCollateral, &0i128);
        Ok(())
    }

    pub fn deposit(env: Env, owner: Address, amount: i128) -> Result<VaultPosition, VaultError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(VaultError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).unwrap_or_default();
        token::Client::new(&env, &config.collateral_token).transfer(
            &owner,
            env.current_contract_address(),
            &amount,
        );
        position.collateral = position
            .collateral
            .checked_add(amount)
            .ok_or(VaultError::Overflow)?;
        adjust_total(&env, amount)?;
        save_position(&env, &owner, &position);
        env.events()
            .publish((symbol_short!("deposit"), owner), amount);
        Ok(position)
    }

    pub fn withdraw(env: Env, owner: Address, amount: i128) -> Result<VaultPosition, VaultError> {
        owner.require_auth();
        let config = load_config(&env)?;
        if amount <= 0 {
            return Err(VaultError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).ok_or(VaultError::InsufficientCollateral)?;
        let remaining = position
            .collateral
            .checked_sub(amount)
            .ok_or(VaultError::InsufficientCollateral)?;
        if remaining < 0 {
            return Err(VaultError::InsufficientCollateral);
        }
        let reserved = BorrowingPoolClient::new(&env, &config.pool).required_collateral(&owner);
        if remaining < reserved {
            return Err(VaultError::ReservedCollateral);
        }
        position.collateral = remaining;
        adjust_total(&env, -amount)?;
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

    pub fn seize(
        env: Env,
        owner: Address,
        liquidator: Address,
        amount: i128,
    ) -> Result<VaultPosition, VaultError> {
        let config = load_config(&env)?;
        config.pool.require_auth();
        if amount <= 0 {
            return Err(VaultError::InvalidAmount);
        }
        let mut position = load_position(&env, &owner).ok_or(VaultError::InsufficientCollateral)?;
        if amount > position.collateral {
            return Err(VaultError::InsufficientCollateral);
        }
        position.collateral -= amount;
        adjust_total(&env, -amount)?;
        save_position(&env, &owner, &position);
        if amount > 0 {
            token::Client::new(&env, &config.collateral_token).transfer(
                &env.current_contract_address(),
                &liquidator,
                &amount,
            );
        }
        env.events()
            .publish((symbol_short!("seize"), owner, liquidator), amount);
        Ok(position)
    }

    pub fn position(env: Env, owner: Address) -> VaultPosition {
        load_position(&env, &owner).unwrap_or_default()
    }

    pub fn collateral_of(env: Env, owner: Address) -> i128 {
        load_position(&env, &owner)
            .map(|position| position.collateral)
            .unwrap_or(0)
    }

    pub fn total_collateral(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&VaultKey::TotalCollateral)
            .unwrap_or(0)
    }

    pub fn config(env: Env) -> Result<VaultConfig, VaultError> {
        load_config(&env)
    }
}

fn load_config(env: &Env) -> Result<VaultConfig, VaultError> {
    env.storage()
        .instance()
        .get(&VaultKey::Config)
        .ok_or(VaultError::NotInitialized)
}

fn load_position(env: &Env, owner: &Address) -> Option<VaultPosition> {
    env.storage()
        .persistent()
        .get(&VaultKey::Position(owner.clone()))
}

fn save_position(env: &Env, owner: &Address, position: &VaultPosition) {
    let key = VaultKey::Position(owner.clone());
    if position.collateral == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, position);
    }
}

fn adjust_total(env: &Env, delta: i128) -> Result<(), VaultError> {
    let current: i128 = env
        .storage()
        .instance()
        .get(&VaultKey::TotalCollateral)
        .unwrap_or(0);
    let next = current.checked_add(delta).ok_or(VaultError::Overflow)?;
    env.storage()
        .instance()
        .set(&VaultKey::TotalCollateral, &next);
    Ok(())
}
