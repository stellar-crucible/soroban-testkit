//! A staking pool with a separately funded reward distributor.
//!
//! The pool keeps one accumulator — reward stroops per staked stroop, scaled by
//! [`SCALE`] — and each position remembers the accumulator it was last paid
//! against. A position's claim is therefore `amount * (pool - paid) / SCALE`,
//! which is the standard way to distribute a continuous reward stream without
//! looping over stakers, and the reason every deposit and withdrawal has to
//! settle the accumulator first.
//!
//! Rewards are minted into the contract by the distributor; the pool itself
//! never mints, so a claim larger than the balance left is a configuration bug
//! and transfers fail rather than inventing value.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, token, Address, Env, Symbol};

/// Fixed-point scale for the reward-per-token accumulator.
pub const SCALE: i128 = 1_000_000_000_000;

/// Basis points in one whole unit.
pub const BPS: i128 = 10_000;

/// Penalty taken from a withdrawal made before the lock elapses: 10%.
pub const EARLY_EXIT_PENALTY_BPS: u32 = 1_000;

/// How long a deposit is locked before it can leave without a penalty.
pub const MIN_LOCK_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Pool-wide state, kept in instance storage.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Pool {
    pub admin: Address,
    pub stake_token: Address,
    pub reward_token: Address,
    /// Reward stroops the distributor pays the pool every second.
    pub reward_rate: i128,
    /// Reward stroops per staked stroop, scaled by [`SCALE`].
    pub reward_per_token: i128,
    pub last_update: u64,
    pub total_staked: i128,
    pub paused: bool,
}

/// One staker's position, kept in persistent storage.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Position {
    pub amount: i128,
    pub reward_per_token_paid: i128,
    /// Rewards settled by an earlier `stake`, `unstake` or `claim`.
    pub settled: i128,
    /// When the current lock started, used for the early-exit penalty.
    pub since: u64,
}

fn pool_key(env: &Env) -> Symbol {
    Symbol::new(env, "pool")
}

fn position_key(env: &Env, owner: &Address) -> (Symbol, Address) {
    (Symbol::new(env, "position"), owner.clone())
}

fn load_pool(env: &Env) -> Pool {
    env.storage()
        .instance()
        .get(&pool_key(env))
        .expect("pool not initialized")
}

fn save_pool(env: &Env, pool: &Pool) {
    env.storage().instance().set(&pool_key(env), pool);
}

fn load_position(env: &Env, owner: &Address) -> Option<Position> {
    env.storage().persistent().get(&position_key(env, owner))
}

fn save_position(env: &Env, owner: &Address, position: &Position) {
    env.storage()
        .persistent()
        .set(&position_key(env, owner), position);
}

/// Bring the accumulator up to `now`, leaving positions untouched.
///
/// An empty pool still moves `last_update` forward: rewards that accrued while
/// nobody was staked belong to nobody, and leaving the timestamp behind would
/// pay them out to the first staker.
fn accrue(env: &Env, pool: &mut Pool) {
    let now = env.ledger().timestamp();
    if now <= pool.last_update {
        return;
    }
    if pool.total_staked > 0 && pool.reward_rate > 0 {
        let elapsed = i128::from(now - pool.last_update);
        let reward = pool.reward_rate * elapsed;
        pool.reward_per_token += reward * SCALE / pool.total_staked;
    }
    pool.last_update = now;
}

/// What `position` has earned against the current accumulator.
fn pending(pool: &Pool, position: &Position) -> i128 {
    let delta = pool.reward_per_token - position.reward_per_token_paid;
    position.settled + position.amount * delta / SCALE
}

/// Move a position's earned rewards into `settled` and re-anchor it.
fn settle(pool: &Pool, position: &mut Position) {
    position.settled = pending(pool, position);
    position.reward_per_token_paid = pool.reward_per_token;
}

fn penalty(amount: i128) -> i128 {
    amount * i128::from(EARLY_EXIT_PENALTY_BPS) / BPS
}

#[contract]
pub struct StakingPool;

#[contractimpl]
impl StakingPool {
    /// Configure a pool whose reward stream is `reward_rate` stroops per second.
    pub fn initialize(
        env: Env,
        admin: Address,
        stake_token: Address,
        reward_token: Address,
        reward_rate: i128,
    ) {
        admin.require_auth();
        assert!(
            env.storage()
                .instance()
                .get::<_, Pool>(&pool_key(&env))
                .is_none(),
            "already initialized"
        );
        assert!(reward_rate >= 0, "reward rate must not be negative");
        save_pool(
            &env,
            &Pool {
                admin,
                stake_token,
                reward_token,
                reward_rate,
                reward_per_token: 0,
                last_update: env.ledger().timestamp(),
                total_staked: 0,
                paused: false,
            },
        );
    }

    pub fn stake(env: Env, owner: Address, amount: i128) {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");
        let mut pool = load_pool(&env);
        assert!(!pool.paused, "pool is paused");

        accrue(&env, &mut pool);
        let mut position = load_position(&env, &owner).unwrap_or(Position {
            amount: 0,
            reward_per_token_paid: pool.reward_per_token,
            settled: 0,
            since: env.ledger().timestamp(),
        });
        settle(&pool, &mut position);

        let stake_token = token::Client::new(&env, &pool.stake_token);
        stake_token.transfer(&owner, env.current_contract_address(), &amount);

        position.amount += amount;
        position.since = env.ledger().timestamp();
        pool.total_staked += amount;
        save_position(&env, &owner, &position);
        save_pool(&env, &pool);
    }

    /// Withdraw `amount`, settling rewards first.
    ///
    /// A withdrawal before [`MIN_LOCK_SECONDS`] have passed since the deposit
    /// pays [`EARLY_EXIT_PENALTY_BPS`] of the withdrawn amount to the admin.
    /// The penalty is taken from principal, not from the staker's rewards.
    pub fn unstake(env: Env, owner: Address, amount: i128) -> i128 {
        owner.require_auth();
        assert!(amount > 0, "amount must be positive");
        let mut pool = load_pool(&env);
        accrue(&env, &mut pool);

        let mut position = load_position(&env, &owner).expect("no stake");
        assert!(amount <= position.amount, "amount exceeds stake");
        settle(&pool, &mut position);

        let now = env.ledger().timestamp();
        let locked = now < position.since.saturating_add(MIN_LOCK_SECONDS);
        let penalty = if locked { penalty(amount) } else { 0 };
        let net = amount - penalty;

        let stake_token = token::Client::new(&env, &pool.stake_token);
        stake_token.transfer(&env.current_contract_address(), &owner, &net);
        if penalty > 0 {
            stake_token.transfer(&env.current_contract_address(), &pool.admin, &penalty);
        }

        position.amount -= amount;
        if position.amount == 0 {
            position.since = now;
        }
        pool.total_staked -= amount;
        save_position(&env, &owner, &position);
        save_pool(&env, &pool);
        net
    }

    pub fn claim(env: Env, owner: Address) -> i128 {
        owner.require_auth();
        let mut pool = load_pool(&env);
        accrue(&env, &mut pool);

        let mut position = load_position(&env, &owner).expect("no stake");
        settle(&pool, &mut position);
        let reward = position.settled;
        position.settled = 0;

        if reward > 0 {
            token::Client::new(&env, &pool.reward_token).transfer(
                &env.current_contract_address(),
                &owner,
                &reward,
            );
        }
        save_position(&env, &owner, &position);
        save_pool(&env, &pool);
        reward
    }

    /// Withdraw the whole principal while the pool is paused, forfeiting
    /// unsettled rewards. New stakes are refused while paused, but exits never
    /// are.
    pub fn emergency_withdraw(env: Env, owner: Address) -> i128 {
        owner.require_auth();
        let mut pool = load_pool(&env);
        assert!(pool.paused, "pool is not paused");
        accrue(&env, &mut pool);

        let mut position = load_position(&env, &owner).expect("no stake");
        assert!(position.amount > 0, "nothing staked");
        settle(&pool, &mut position);

        let amount = position.amount;
        let penalty = penalty(amount);
        let net = amount - penalty;
        let stake_token = token::Client::new(&env, &pool.stake_token);
        stake_token.transfer(&env.current_contract_address(), &owner, &net);
        stake_token.transfer(&env.current_contract_address(), &pool.admin, &penalty);

        position.amount = 0;
        position.settled = 0;
        position.since = env.ledger().timestamp();
        pool.total_staked -= amount;
        save_position(&env, &owner, &position);
        save_pool(&env, &pool);
        net
    }

    /// Change the reward stream. Rewards already accrued are settled first, so
    /// a rate change cannot retroactively pay at the new rate.
    pub fn set_reward_rate(env: Env, reward_rate: i128) {
        let mut pool = load_pool(&env);
        pool.admin.require_auth();
        assert!(reward_rate >= 0, "reward rate must not be negative");
        accrue(&env, &mut pool);
        pool.reward_rate = reward_rate;
        save_pool(&env, &pool);
    }

    pub fn set_paused(env: Env, paused: bool) {
        let mut pool = load_pool(&env);
        pool.admin.require_auth();
        accrue(&env, &mut pool);
        pool.paused = paused;
        save_pool(&env, &pool);
    }

    // --- views -----------------------------------------------------------

    pub fn total_staked(env: Env) -> i128 {
        load_pool(&env).total_staked
    }

    pub fn staked(env: Env, owner: Address) -> i128 {
        load_position(&env, &owner).map_or(0, |position| position.amount)
    }

    /// What `owner` was last settled for plus what has accrued since.
    pub fn earned(env: Env, owner: Address) -> i128 {
        let pool = load_pool(&env);
        match load_position(&env, &owner) {
            Some(position) => pending(&project(&env, &pool), &position),
            None => 0,
        }
    }

    pub fn reward_rate(env: Env) -> i128 {
        load_pool(&env).reward_rate
    }

    pub fn is_paused(env: Env) -> bool {
        load_pool(&env).paused
    }

    pub fn pool(env: Env) -> Pool {
        load_pool(&env)
    }
}

/// The pool as it would be if the accumulator were settled now, without writing.
fn project(env: &Env, pool: &Pool) -> Pool {
    let mut projected = pool.clone();
    accrue(env, &mut projected);
    projected
}
