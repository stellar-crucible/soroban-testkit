use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum SynthError {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    InvalidAmount = 3,
    InsufficientBalance = 4,
    Overflow = 5,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
enum SynthKey {
    Minter,
    TotalSupply,
    Balance(Address),
}

/// The synthetic USD token. Supply changes only through its minter, which in
/// this protocol is the [`CdpEngine`](crate::engine::CdpEngine).
#[contract]
pub struct SyntheticToken;

#[contractimpl]
impl SyntheticToken {
    pub fn initialize(env: Env, minter: Address) -> Result<(), SynthError> {
        if env.storage().instance().has(&SynthKey::Minter) {
            return Err(SynthError::AlreadyInitialized);
        }
        env.storage().instance().set(&SynthKey::Minter, &minter);
        env.storage().instance().set(&SynthKey::TotalSupply, &0i128);
        Ok(())
    }

    /// Create `amount` new tokens for `to`. Minter only.
    pub fn mint(env: Env, to: Address, amount: i128) -> Result<(), SynthError> {
        require_minter(&env)?;
        if amount <= 0 {
            return Err(SynthError::InvalidAmount);
        }
        let balance = balance_of(&env, &to)
            .checked_add(amount)
            .ok_or(SynthError::Overflow)?;
        let supply = total_supply(&env)
            .checked_add(amount)
            .ok_or(SynthError::Overflow)?;
        set_balance(&env, &to, balance);
        env.storage()
            .instance()
            .set(&SynthKey::TotalSupply, &supply);
        env.events().publish((symbol_short!("mint"), to), amount);
        Ok(())
    }

    /// Destroy `amount` of `from`'s tokens. Minter only: the engine burns on a
    /// repayment or a liquidation after checking the payer's own authorization.
    pub fn burn(env: Env, from: Address, amount: i128) -> Result<(), SynthError> {
        require_minter(&env)?;
        if amount <= 0 {
            return Err(SynthError::InvalidAmount);
        }
        let balance = balance_of(&env, &from);
        if balance < amount {
            return Err(SynthError::InsufficientBalance);
        }
        set_balance(&env, &from, balance - amount);
        env.storage()
            .instance()
            .set(&SynthKey::TotalSupply, &(total_supply(&env) - amount));
        env.events().publish((symbol_short!("burn"), from), amount);
        Ok(())
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) -> Result<(), SynthError> {
        from.require_auth();
        if amount <= 0 {
            return Err(SynthError::InvalidAmount);
        }
        let from_balance = balance_of(&env, &from);
        if from_balance < amount {
            return Err(SynthError::InsufficientBalance);
        }
        set_balance(&env, &from, from_balance - amount);
        let to_balance = balance_of(&env, &to)
            .checked_add(amount)
            .ok_or(SynthError::Overflow)?;
        set_balance(&env, &to, to_balance);
        env.events()
            .publish((symbol_short!("transfer"), from, to), amount);
        Ok(())
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        balance_of(&env, &id)
    }

    pub fn total_supply(env: Env) -> i128 {
        total_supply(&env)
    }

    pub fn minter(env: Env) -> Result<Address, SynthError> {
        env.storage()
            .instance()
            .get(&SynthKey::Minter)
            .ok_or(SynthError::NotInitialized)
    }
}

fn require_minter(env: &Env) -> Result<(), SynthError> {
    let minter: Address = env
        .storage()
        .instance()
        .get(&SynthKey::Minter)
        .ok_or(SynthError::NotInitialized)?;
    minter.require_auth();
    Ok(())
}

fn balance_of(env: &Env, id: &Address) -> i128 {
    env.storage()
        .persistent()
        .get(&SynthKey::Balance(id.clone()))
        .unwrap_or(0)
}

fn set_balance(env: &Env, id: &Address, amount: i128) {
    let key = SynthKey::Balance(id.clone());
    if amount == 0 {
        env.storage().persistent().remove(&key);
    } else {
        env.storage().persistent().set(&key, &amount);
    }
}

fn total_supply(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&SynthKey::TotalSupply)
        .unwrap_or(0)
}
