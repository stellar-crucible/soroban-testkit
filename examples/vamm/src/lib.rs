#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, Address, Env, Map, Symbol};

pub const SCALE: i128 = 10_000_000;
pub const MAINTENANCE_MARGIN_BP: i128 = 1_000;

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position {
    pub trader: Address,
    pub size: i128,
    pub collateral: i128,
    pub entry_mark: i128,
    pub leverage: i32,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VammState {
    pub virtual_base: i128,
    pub virtual_quote: i128,
    pub funding_index: i128,
    pub last_funding: u64,
}

fn state_key(env: &Env) -> Symbol {
    Symbol::new(env, "vamm")
}

fn positions_key(env: &Env) -> Symbol {
    Symbol::new(env, "positions")
}

fn state(env: &Env) -> VammState {
    env.storage()
        .instance()
        .get(&state_key(env))
        .unwrap_or(VammState {
            virtual_base: SCALE,
            virtual_quote: SCALE,
            funding_index: 0,
            last_funding: env.ledger().timestamp(),
        })
}

fn write_state(env: &Env, value: &VammState) {
    env.storage().instance().set(&state_key(env), value);
}

fn positions(env: &Env) -> Map<Address, Position> {
    env.storage()
        .persistent()
        .get(&positions_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_positions(env: &Env, value: &Map<Address, Position>) {
    env.storage().persistent().set(&positions_key(env), value);
}

pub fn mark_price(state: &VammState) -> i128 {
    state.virtual_quote.saturating_mul(SCALE) / state.virtual_base.max(1)
}

pub fn unrealized_pnl(position: &Position, mark: i128) -> i128 {
    if position.size == 0 || position.entry_mark == 0 {
        return 0;
    }
    let price_delta = mark.saturating_sub(position.entry_mark);
    position
        .size
        .saturating_mul(price_delta)
        .saturating_div(SCALE)
}

pub fn liquidation_required(position: &Position, mark: i128) -> bool {
    let equity = position
        .collateral
        .saturating_add(unrealized_pnl(position, mark));
    let maintenance = position
        .size
        .abs()
        .saturating_mul(mark)
        .saturating_div(SCALE)
        .saturating_mul(MAINTENANCE_MARGIN_BP)
        .saturating_div(10_000);
    equity <= maintenance
}

#[contract]
pub struct VammContract;

#[contractimpl]
impl VammContract {
    pub fn initialize(env: Env, virtual_base: i128, virtual_quote: i128) {
        assert!(virtual_base > 0, "virtual base must be positive");
        assert!(virtual_quote > 0, "virtual quote must be positive");
        write_state(
            &env,
            &VammState {
                virtual_base,
                virtual_quote,
                funding_index: 0,
                last_funding: env.ledger().timestamp(),
            },
        );
    }

    pub fn mark(env: Env) -> i128 {
        mark_price(&state(&env))
    }

    pub fn open_position(
        env: Env,
        trader: Address,
        side: i32,
        collateral: i128,
        leverage: i32,
    ) -> Position {
        trader.require_auth();
        assert!(side == 1 || side == -1, "side must be long or short");
        assert!(collateral > 0, "collateral must be positive");
        assert!((1..=10).contains(&leverage), "leverage must be 1 to 10");

        let mut all = positions(&env);
        assert!(!all.contains_key(trader.clone()), "position already open");
        let current = state(&env);
        let mark = mark_price(&current);
        let notional = collateral.saturating_mul(i128::from(leverage));
        let size = notional.saturating_mul(SCALE).saturating_div(mark);
        let signed_size = if side == 1 { size } else { -size };
        let position = Position {
            trader: trader.clone(),
            size: signed_size,
            collateral,
            entry_mark: mark,
            leverage,
        };
        all.set(trader, position.clone());
        write_positions(&env, &all);
        position
    }

    pub fn position(env: Env, trader: Address) -> Option<Position> {
        positions(&env).get(trader)
    }

    pub fn add_margin(env: Env, trader: Address, amount: i128) -> Position {
        trader.require_auth();
        assert!(amount > 0, "margin must be positive");
        let mut all = positions(&env);
        let mut position = all.get(trader.clone()).expect("position not found");
        position.collateral = position.collateral.saturating_add(amount);
        all.set(trader, position.clone());
        write_positions(&env, &all);
        position
    }

    pub fn close_position(env: Env, trader: Address) -> i128 {
        trader.require_auth();
        let mut all = positions(&env);
        let position = all.get(trader.clone()).expect("position not found");
        let mark = mark_price(&state(&env));
        let pnl = unrealized_pnl(&position, mark);
        all.remove(trader);
        write_positions(&env, &all);
        position.collateral.saturating_add(pnl)
    }

    pub fn is_liquidatable(env: Env, trader: Address) -> bool {
        let position = positions(&env).get(trader).expect("position not found");
        liquidation_required(&position, mark_price(&state(&env)))
    }

    pub fn liquidate(env: Env, trader: Address) -> i128 {
        let position = positions(&env)
            .get(trader.clone())
            .expect("position not found");
        let mark = mark_price(&state(&env));
        assert!(liquidation_required(&position, mark), "position is healthy");
        let mut all = positions(&env);
        all.remove(trader);
        write_positions(&env, &all);
        0
    }

    pub fn move_mark(env: Env, virtual_base: i128, virtual_quote: i128) {
        assert!(virtual_base > 0, "virtual base must be positive");
        assert!(virtual_quote > 0, "virtual quote must be positive");
        let mut current = state(&env);
        current.virtual_base = virtual_base;
        current.virtual_quote = virtual_quote;
        write_state(&env, &current);
    }

    pub fn set_funding(env: Env, funding_index: i128) {
        let mut current = state(&env);
        current.funding_index = funding_index;
        current.last_funding = env.ledger().timestamp();
        write_state(&env, &current);
    }

    pub fn funding_index(env: Env) -> i128 {
        state(&env).funding_index
    }

    pub fn unrealized_pnl(env: Env, trader: Address) -> i128 {
        let position = positions(&env).get(trader).expect("position not found");
        unrealized_pnl(&position, mark_price(&state(&env)))
    }
}
