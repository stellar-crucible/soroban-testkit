#![no_std]

use soroban_sdk::{contract, contractevent, contractimpl, Address, Env, Symbol};

#[contract]
pub struct CounterContract;

#[contractevent(topics = ["counter", "incremented"])]
#[derive(Clone)]
pub struct Incremented {
    pub caller: Address,
    pub new_count: u32,
}

#[contractimpl]
impl CounterContract {
    /// Reads the current counter value from instance storage.
    pub fn get(env: Env) -> u32 {
        let key = Symbol::new(&env, "count");
        env.storage().instance().get(&key).unwrap_or(0)
    }

    /// Increases the counter by `by`, records the caller, and emits an event.
    pub fn increment(env: Env, caller: Address, by: u32) -> u32 {
        caller.require_auth();

        let key = Symbol::new(&env, "count");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0);
        let next = current.checked_add(by).expect("counter overflow");
        env.storage().instance().set(&key, &next);

        Incremented {
            caller,
            new_count: next,
        }
        .publish(&env);

        next
    }
}

#[cfg(test)]
mod test;
