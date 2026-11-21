use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, symbol_short, Address,
    Env, String,
};

/// Token errors start at 100 so they never collide with [`crate::AmmError`]
/// codes when a token failure propagates out through the pool.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum TokenError {
    NegativeAmount = 100,
    InsufficientBalance = 101,
    InsufficientAllowance = 102,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenKey {
    Admin,
    Decimals,
    Name,
    Symbol,
    Supply,
    Balance(Address),
    Allowance(Address, Address),
}

/// An ERC-20 shaped token: balances and allowances in persistent storage,
/// metadata and supply in instance storage.
#[contract]
pub struct AmmToken;

#[contractimpl]
impl AmmToken {
    pub fn __constructor(env: Env, admin: Address, decimals: u32, name: String, symbol: String) {
        let instance = env.storage().instance();
        instance.set(&TokenKey::Admin, &admin);
        instance.set(&TokenKey::Decimals, &decimals);
        instance.set(&TokenKey::Name, &name);
        instance.set(&TokenKey::Symbol, &symbol);
        instance.set(&TokenKey::Supply, &0i128);
    }

    pub fn mint(env: Env, to: Address, amount: i128) {
        Self::admin(env.clone()).require_auth();
        non_negative(&env, amount);
        Self::credit(&env, &to, amount);
        let supply: i128 = env.storage().instance().get(&TokenKey::Supply).unwrap_or(0);
        env.storage()
            .instance()
            .set(&TokenKey::Supply, &(supply + amount));
        env.events().publish((symbol_short!("mint"), to), amount);
    }

    pub fn burn(env: Env, from: Address, amount: i128) {
        from.require_auth();
        non_negative(&env, amount);
        Self::debit(&env, &from, amount);
        let supply: i128 = env.storage().instance().get(&TokenKey::Supply).unwrap_or(0);
        env.storage()
            .instance()
            .set(&TokenKey::Supply, &(supply - amount));
        env.events().publish((symbol_short!("burn"), from), amount);
    }

    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        non_negative(&env, amount);
        Self::debit(&env, &from, amount);
        Self::credit(&env, &to, amount);
        env.events()
            .publish((symbol_short!("transfer"), from, to), amount);
    }

    pub fn approve(env: Env, from: Address, spender: Address, amount: i128) {
        from.require_auth();
        non_negative(&env, amount);
        env.storage()
            .persistent()
            .set(&TokenKey::Allowance(from.clone(), spender.clone()), &amount);
        env.events()
            .publish((symbol_short!("approve"), from, spender), amount);
    }

    pub fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        non_negative(&env, amount);
        let key = TokenKey::Allowance(from.clone(), spender);
        let allowed: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if allowed < amount {
            panic_with_error!(&env, TokenError::InsufficientAllowance);
        }
        env.storage().persistent().set(&key, &(allowed - amount));
        Self::debit(&env, &from, amount);
        Self::credit(&env, &to, amount);
        env.events()
            .publish((symbol_short!("transfer"), from, to), amount);
    }

    pub fn balance(env: Env, id: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&TokenKey::Balance(id))
            .unwrap_or(0)
    }

    pub fn allowance(env: Env, from: Address, spender: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&TokenKey::Allowance(from, spender))
            .unwrap_or(0)
    }

    pub fn total_supply(env: Env) -> i128 {
        env.storage().instance().get(&TokenKey::Supply).unwrap_or(0)
    }

    pub fn admin(env: Env) -> Address {
        env.storage().instance().get(&TokenKey::Admin).unwrap()
    }

    pub fn decimals(env: Env) -> u32 {
        env.storage().instance().get(&TokenKey::Decimals).unwrap()
    }

    pub fn name(env: Env) -> String {
        env.storage().instance().get(&TokenKey::Name).unwrap()
    }

    pub fn symbol(env: Env) -> String {
        env.storage().instance().get(&TokenKey::Symbol).unwrap()
    }
}

impl AmmToken {
    fn credit(env: &Env, to: &Address, amount: i128) {
        let key = TokenKey::Balance(to.clone());
        let balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(balance + amount));
    }

    fn debit(env: &Env, from: &Address, amount: i128) {
        let key = TokenKey::Balance(from.clone());
        let balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if balance < amount {
            panic_with_error!(env, TokenError::InsufficientBalance);
        }
        env.storage().persistent().set(&key, &(balance - amount));
    }
}

fn non_negative(env: &Env, amount: i128) {
    if amount < 0 {
        panic_with_error!(env, TokenError::NegativeAmount);
    }
}
