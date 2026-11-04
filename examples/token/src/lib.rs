#![no_std]

use soroban_sdk::{contract, contractevent, contractimpl, Address, Env, Map, Symbol};

/// An ERC-20 shaped token: a supply that only moves through `mint`, `burn` and
/// `transfer`, balances keyed by owner, and allowances a spender draws down.
///
/// It is deliberately small. Everything the counter example cannot show — a data
/// payload with three fields in it, more than one contract registered in the same
/// environment, and a property test over generated addresses — lives here.
#[contract]
pub struct TokenContract;

/// The usual ERC-20 transfer. `from`, `to` and `amount` travel in the event
/// data, so a test can assert the value that moved rather than only that
/// something moved.
#[contractevent(topics = ["token", "transfer"])]
#[derive(Clone)]
pub struct Transfer {
    pub from: Address,
    pub to: Address,
    pub amount: i128,
}

/// A permission grant. The three fields are what a test needs to check that an
/// allowance was set rather than merely that `approve` returned.
#[contractevent(topics = ["token", "approve"])]
#[derive(Clone)]
pub struct Approval {
    pub owner: Address,
    pub spender: Address,
    pub amount: i128,
}

/// Minting has no `from`, and the SDK publishes event maps sparse, so this
/// payload carries two fields instead of three.
#[contractevent(topics = ["token", "mint"])]
#[derive(Clone)]
pub struct Minted {
    pub to: Address,
    pub amount: i128,
}

/// Burning has no `to`, for the same reason minting has no `from`: one side of
/// the move is the supply itself, and it is not an address.
#[contractevent(topics = ["token", "burn"])]
#[derive(Clone)]
pub struct Burnt {
    pub from: Address,
    pub amount: i128,
}

fn admin_key(env: &Env) -> Symbol {
    Symbol::new(env, "admin")
}

fn supply_key(env: &Env) -> Symbol {
    Symbol::new(env, "supply")
}

fn balances_key(env: &Env) -> Symbol {
    Symbol::new(env, "balances")
}

fn allowances_key(env: &Env) -> Symbol {
    Symbol::new(env, "allowances")
}

fn balances(env: &Env) -> Map<Address, i128> {
    env.storage()
        .persistent()
        .get(&balances_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_balances(env: &Env, map: &Map<Address, i128>) {
    env.storage().persistent().set(&balances_key(env), map);
}

fn allowances(env: &Env) -> Map<(Address, Address), i128> {
    env.storage()
        .persistent()
        .get(&allowances_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_allowances(env: &Env, map: &Map<(Address, Address), i128>) {
    env.storage().persistent().set(&allowances_key(env), map);
}

fn supply(env: &Env) -> i128 {
    env.storage().instance().get(&supply_key(env)).unwrap_or(0)
}

fn set_supply(env: &Env, value: i128) {
    env.storage().instance().set(&supply_key(env), &value);
}

fn move_balance(env: &Env, from: &Address, to: &Address, amount: i128) {
    let mut map = balances(env);
    let sent = map.get(from.clone()).unwrap_or(0);
    assert!(sent >= amount, "insufficient balance");
    let received = map.get(to.clone()).unwrap_or(0);
    map.set(from.clone(), sent - amount);
    map.set(to.clone(), received + amount);
    write_balances(env, &map);
}

#[contractimpl]
impl TokenContract {
    /// Records the mint authority. Call it once, before any mint.
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        let key = admin_key(&env);
        assert!(
            !env.storage().instance().has(&key),
            "token already initialized"
        );
        env.storage().instance().set(&key, &admin);
        set_supply(&env, 0);
    }

    /// The admin recorded by `initialize`.
    pub fn admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&admin_key(&env))
            .expect("token not initialized")
    }

    /// The supply that every mint and burn so far has produced.
    pub fn total_supply(env: Env) -> i128 {
        supply(&env)
    }

    /// What `owner` holds. A holder that has never received a token holds zero.
    pub fn balance_of(env: Env, owner: Address) -> i128 {
        balances(&env).get(owner).unwrap_or(0)
    }

    /// How much more `spender` may still move out of `owner`'s balance.
    pub fn allowance(env: Env, owner: Address, spender: Address) -> i128 {
        allowances(&env).get((owner, spender)).unwrap_or(0)
    }

    /// Creates `amount` new tokens for `to` and returns the supply they were
    /// added to. The admin's authorization is what makes it happen.
    pub fn mint(env: Env, to: Address, amount: i128) -> i128 {
        Self::admin(env.clone()).require_auth();

        let mut map = balances(&env);
        let held = map.get(to.clone()).unwrap_or(0);
        map.set(to.clone(), held + amount);
        write_balances(&env, &map);

        let next = supply(&env) + amount;
        set_supply(&env, next);

        Minted { to, amount }.publish(&env);
        next
    }

    /// Removes `amount` from `from`'s balance and out of the supply.
    pub fn burn(env: Env, from: Address, amount: i128) -> i128 {
        from.require_auth();

        let mut map = balances(&env);
        let held = map.get(from.clone()).unwrap_or(0);
        assert!(held >= amount, "insufficient balance");
        map.set(from.clone(), held - amount);
        write_balances(&env, &map);

        let next = supply(&env) - amount;
        set_supply(&env, next);

        Burnt { from, amount }.publish(&env);
        next
    }

    /// Moves `amount` between two holders. `from` must authorize it.
    pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        move_balance(&env, &from, &to, amount);
        Transfer { from, to, amount }.publish(&env);
    }

    /// Lets `spender` move up to `amount` out of `owner`'s balance.
    pub fn approve(env: Env, owner: Address, spender: Address, amount: i128) {
        owner.require_auth();

        let mut map = allowances(&env);
        map.set((owner.clone(), spender.clone()), amount);
        write_allowances(&env, &map);

        Approval {
            owner,
            spender,
            amount,
        }
        .publish(&env);
    }

    /// Spends an allowance: `spender` moves `amount` from `from` to `to` and
    /// draws the allowance down by exactly what it used.
    pub fn transfer_from(env: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();

        let left = allowances(&env)
            .get((from.clone(), spender.clone()))
            .unwrap_or(0);
        assert!(left >= amount, "insufficient allowance");

        move_balance(&env, &from, &to, amount);

        let mut map = allowances(&env);
        map.set((from.clone(), spender.clone()), left - amount);
        write_allowances(&env, &map);

        Transfer { from, to, amount }.publish(&env);
    }
}

#[cfg(test)]
mod test;
