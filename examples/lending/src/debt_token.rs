use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum DebtTokenError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidAmount = 3,
    InsufficientBalance = 4,
    Overflow = 5,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum DebtTokenKey {
    Minter,
    TotalSupply,
    Balance(Address),
}

#[contract]
pub struct DebtToken;

#[contractimpl]
impl DebtToken {
    pub fn initialize(env: Env, minter: Address) -> Result<(), DebtTokenError> {
        if env.storage().instance().has(&DebtTokenKey::Minter) {
            return Err(DebtTokenError::AlreadyInitialized);
        }
        env.storage().instance().set(&DebtTokenKey::Minter, &minter);
        env.storage()
            .instance()
            .set(&DebtTokenKey::TotalSupply, &0i128);
        Ok(())
    }

    pub fn mint(env: Env, to: Address, amount: i128) -> Result<(), DebtTokenError> {
        require_minter(&env)?;
        if amount <= 0 {
            return Err(DebtTokenError::InvalidAmount);
        }
        let balance = balance_of(&env, &to)
            .checked_add(amount)
            .ok_or(DebtTokenError::Overflow)?;
        let supply = total_supply(&env)
            .checked_add(amount)
            .ok_or(DebtTokenError::Overflow)?;
        set_balance(&env, &to, balance);
        env.storage()
            .instance()
            .set(&DebtTokenKey::TotalSupply, &supply);
        env.events().publish((symbol_short!("mint"), to), amount);
        Ok(())
    }

    pub fn burn(env: Env, from: Address, amount: i128) -> Result<(), DebtTokenError> {
        require_minter(&env)?;
        if amount <= 0 {
            return Err(DebtTokenError::InvalidAmount);
        }
        let balance = balance_of(&env, &from);
        if balance < amount {
            return Err(DebtTokenError::InsufficientBalance);
        }
        set_balance(&env, &from, balance - amount);
        env.storage()
            .instance()
            .set(&DebtTokenKey::TotalSupply, &(total_supply(&env) - amount));
        env.events().publish((symbol_short!("burn"), from), amount);
        Ok(())
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        balance_of(&env, &id)
    }

    pub fn total_supply(env: Env) -> i128 {
        total_supply(&env)
    }

    pub fn minter(env: Env) -> Result<Address, DebtTokenError> {
        env.storage()
            .instance()
            .get(&DebtTokenKey::Minter)
            .ok_or(DebtTokenError::NotInitialized)
    }
}

fn require_minter(env: &Env) -> Result<(), DebtTokenError> {
    let minter: Address = env
        .storage()
        .instance()
        .get(&DebtTokenKey::Minter)
        .ok_or(DebtTokenError::NotInitialized)?;
    minter.require_auth();
    Ok(())
}

fn balance_of(env: &Env, id: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&DebtTokenKey::Balance(id.clone()))
        .unwrap_or(0)
}

fn set_balance(env: &Env, id: &Address, amount: i128) {
    let key = DebtTokenKey::Balance(id.clone());
    if amount == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &amount);
    }
}

fn total_supply(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DebtTokenKey::TotalSupply)
        .unwrap_or(0)
}
