#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, Map, Symbol};

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Pool {
    pub admin: Address,
    pub asset: Address,
    pub total_shares: i128,
    pub total_assets: i128,
    pub balances: Map<Address, i128>,
}

fn pool_key(env: &Env) -> Symbol { Symbol::new(env, "pool") }
fn load(env: &Env) -> Pool { env.storage().instance().get(&pool_key(env)).expect("pool not initialized") }
fn save(env: &Env, pool: &Pool) { env.storage().instance().set(&pool_key(env), pool); }

#[contract]
pub struct LiquidStaking;

#[contractimpl]
impl LiquidStaking {
    pub fn initialize(env: Env, admin: Address, asset: Address) {
        admin.require_auth();
        assert!(env.storage().instance().get::<_, Pool>(&pool_key(&env)).is_none(), "already initialized");
        save(&env, &Pool { admin, asset, total_shares: 0, total_assets: 0, balances: Map::new(&env) });
    }

    pub fn deposit(env: Env, owner: Address, amount: i128) -> i128 {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");
        let mut pool = load(&env);
        let shares = if pool.total_shares == 0 { amount } else {
            amount.checked_mul(pool.total_shares).expect("share overflow") / pool.total_assets
        };
        assert!(shares > 0, "deposit too small");
        token::Client::new(&env, &pool.asset).transfer(&owner, &env.current_contract_address(), &amount);
        pool.total_assets += amount;
        pool.total_shares += shares;
        let old = pool.balances.get(owner.clone()).unwrap_or(0);
        pool.balances.set(owner, old + shares);
        save(&env, &pool);
        shares
    }

    /// Test/operations hook representing newly accrued staking rewards.
    pub fn accrue_yield(env: Env, admin: Address, amount: i128) {
        admin.require_auth();
        let mut pool = load(&env);
        assert_eq!(admin, pool.admin, "not admin");
        assert!(amount >= 0, "yield cannot be negative");
        token::Client::new(&env, &pool.asset).transfer(&admin, &env.current_contract_address(), &amount);
        pool.total_assets += amount;
        save(&env, &pool);
    }

    pub fn redeem(env: Env, owner: Address, shares: i128) -> i128 {
        owner.require_auth();
        assert!(shares > 0, "shares must be positive");
        let mut pool = load(&env);
        let owned = pool.balances.get(owner.clone()).unwrap_or(0);
        assert!(owned >= shares, "insufficient shares");
        let amount = shares.checked_mul(pool.total_assets).expect("redemption overflow") / pool.total_shares;
        pool.balances.set(owner.clone(), owned - shares);
        pool.total_shares -= shares;
        pool.total_assets -= amount;
        token::Client::new(&env, &pool.asset).transfer(&env.current_contract_address(), &owner, &amount);
        save(&env, &pool);
        amount
    }

    pub fn shares(env: Env, owner: Address) -> i128 { load(&env).balances.get(owner).unwrap_or(0) }
    pub fn total_assets(env: Env) -> i128 { load(&env).total_assets }
    pub fn total_shares(env: Env) -> i128 { load(&env).total_shares }
    pub fn exchange_rate(env: Env) -> i128 {
        let pool = load(&env);
        if pool.total_shares == 0 { 1_000_000 } else { pool.total_assets * 1_000_000 / pool.total_shares }
    }
}
