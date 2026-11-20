#![no_std]
#![allow(deprecated)]

use soroban_sdk::token::TokenClient;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env, Vec,
};

/// 7 decimals precision for price representation (1.0 = 10_000_000).
pub const PRICE_PRECISION: i128 = 10_000_000;

/// Basis points denominator (100% = 10,000 bps).
pub const BPS_DENOMINATOR: i128 = 10_000;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum OrderBookError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    InvalidAmount = 3,
    InvalidPrice = 4,
    OrderNotFound = 5,
    Unauthorized = 6,
    ArithmeticError = 7,
    OrderAlreadyFilled = 8,
    InsufficientBookLiquidity = 9,
}

#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Side {
    Buy = 1,
    Sell = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderBookConfig {
    pub admin: Address,
    pub base_token: Address,
    pub quote_token: Address,
    pub maker_fee_bps: u32,
    pub taker_fee_bps: u32,
    pub next_order_id: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    pub id: u64,
    pub maker: Address,
    pub side: Side,
    pub price: i128,
    pub amount: i128,
    pub filled: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Order(u64),
    Bids,
    Asks,
}

/// Computes the quote payment required for a given base amount and price:
/// `(amount * price) / PRICE_PRECISION`
pub fn calculate_quote_amount(amount: i128, price: i128) -> Option<i128> {
    if amount <= 0 || price <= 0 {
        return None;
    }
    amount.checked_mul(price)?.checked_div(PRICE_PRECISION)
}

#[contract]
pub struct OrderBookContract;

#[contractimpl]
impl OrderBookContract {
    /// Initialize the central limit order book for a given trading pair.
    pub fn initialize(
        env: Env,
        admin: Address,
        base_token: Address,
        quote_token: Address,
        maker_fee_bps: u32,
        taker_fee_bps: u32,
    ) {
        if env.storage().persistent().has(&DataKey::Config) {
            panic!("{:?}", OrderBookError::AlreadyInitialized);
        }

        let config = OrderBookConfig {
            admin,
            base_token,
            quote_token,
            maker_fee_bps,
            taker_fee_bps,
            next_order_id: 1,
        };

        env.storage().persistent().set(&DataKey::Config, &config);
        env.storage()
            .persistent()
            .set(&DataKey::Bids, &Vec::<u64>::new(&env));
        env.storage()
            .persistent()
            .set(&DataKey::Asks, &Vec::<u64>::new(&env));
    }

    /// Place a limit order into the book. Maker funds are escrowed in the contract.
    pub fn place_order(env: Env, maker: Address, side: Side, price: i128, amount: i128) -> u64 {
        maker.require_auth();

        if amount <= 0 {
            panic!("{:?}", OrderBookError::InvalidAmount);
        }
        if price <= 0 {
            panic!("{:?}", OrderBookError::InvalidPrice);
        }

        let mut config: OrderBookConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::NotInitialized));

        let order_id = config.next_order_id;
        config.next_order_id = config
            .next_order_id
            .checked_add(1)
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
        env.storage().persistent().set(&DataKey::Config, &config);

        let contract_addr = env.current_contract_address();

        // Escrow funds from maker
        match side {
            Side::Buy => {
                let quote_required = calculate_quote_amount(amount, price)
                    .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
                if quote_required <= 0 {
                    panic!("{:?}", OrderBookError::InvalidAmount);
                }
                TokenClient::new(&env, &config.quote_token).transfer(
                    &maker,
                    &contract_addr,
                    &quote_required,
                );
            }
            Side::Sell => {
                TokenClient::new(&env, &config.base_token).transfer(
                    &maker,
                    &contract_addr,
                    &amount,
                );
            }
        }

        let now = env.ledger().timestamp();
        let order = Order {
            id: order_id,
            maker: maker.clone(),
            side,
            price,
            amount,
            filled: 0,
            timestamp: now,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Order(order_id), &order);

        // Insert into sorted order book maintaining price-time priority
        match side {
            Side::Buy => {
                let bids: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Bids)
                    .unwrap_or_else(|| Vec::new(&env));
                let mut new_bids = Vec::new(&env);
                let mut inserted = false;

                for i in 0..bids.len() {
                    let existing_id = bids.get(i).unwrap();
                    let existing_order: Order = env
                        .storage()
                        .persistent()
                        .get(&DataKey::Order(existing_id))
                        .unwrap();

                    // Bids sorted descending: highest price first
                    if !inserted && price > existing_order.price {
                        new_bids.push_back(order_id);
                        inserted = true;
                    }
                    new_bids.push_back(existing_id);
                }
                if !inserted {
                    new_bids.push_back(order_id);
                }
                env.storage().persistent().set(&DataKey::Bids, &new_bids);
            }
            Side::Sell => {
                let asks: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Asks)
                    .unwrap_or_else(|| Vec::new(&env));
                let mut new_asks = Vec::new(&env);
                let mut inserted = false;

                for i in 0..asks.len() {
                    let existing_id = asks.get(i).unwrap();
                    let existing_order: Order = env
                        .storage()
                        .persistent()
                        .get(&DataKey::Order(existing_id))
                        .unwrap();

                    // Asks sorted ascending: lowest price first
                    if !inserted && price < existing_order.price {
                        new_asks.push_back(order_id);
                        inserted = true;
                    }
                    new_asks.push_back(existing_id);
                }
                if !inserted {
                    new_asks.push_back(order_id);
                }
                env.storage().persistent().set(&DataKey::Asks, &new_asks);
            }
        }

        env.events().publish(
            (symbol_short!("placed"), maker),
            (order_id, side as u32, price, amount),
        );

        order_id
    }

    /// Cancel an unfilled or partially filled order, refunding remaining escrowed balance.
    pub fn cancel_order(env: Env, maker: Address, order_id: u64) {
        maker.require_auth();

        let order: Order = env
            .storage()
            .persistent()
            .get(&DataKey::Order(order_id))
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::OrderNotFound));

        if order.maker != maker {
            panic!("{:?}", OrderBookError::Unauthorized);
        }

        let unfilled = order
            .amount
            .checked_sub(order.filled)
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

        if unfilled <= 0 {
            panic!("{:?}", OrderBookError::OrderAlreadyFilled);
        }

        let config: OrderBookConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::NotInitialized));

        let contract_addr = env.current_contract_address();

        // Refund remaining escrowed funds to maker
        match order.side {
            Side::Buy => {
                let quote_refund = calculate_quote_amount(unfilled, order.price)
                    .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
                if quote_refund > 0 {
                    TokenClient::new(&env, &config.quote_token).transfer(
                        &contract_addr,
                        &maker,
                        &quote_refund,
                    );
                }
            }
            Side::Sell => {
                TokenClient::new(&env, &config.base_token).transfer(
                    &contract_addr,
                    &maker,
                    &unfilled,
                );
            }
        }

        // Remove from orderbook ID list
        match order.side {
            Side::Buy => {
                let bids: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Bids)
                    .unwrap_or_else(|| Vec::new(&env));
                let mut new_bids = Vec::new(&env);
                for i in 0..bids.len() {
                    let id = bids.get(i).unwrap();
                    if id != order_id {
                        new_bids.push_back(id);
                    }
                }
                env.storage().persistent().set(&DataKey::Bids, &new_bids);
            }
            Side::Sell => {
                let asks: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Asks)
                    .unwrap_or_else(|| Vec::new(&env));
                let mut new_asks = Vec::new(&env);
                for i in 0..asks.len() {
                    let id = asks.get(i).unwrap();
                    if id != order_id {
                        new_asks.push_back(id);
                    }
                }
                env.storage().persistent().set(&DataKey::Asks, &new_asks);
            }
        }

        // Delete order from persistent storage upon cancellation
        env.storage().persistent().remove(&DataKey::Order(order_id));

        env.events()
            .publish((symbol_short!("cancelled"), maker), (order_id, unfilled));
    }

    /// Execute a market order matching against the order book.
    ///
    /// If `taker_side == Side::Buy`: taker buys `amount` base tokens matching against Asks.
    /// If `taker_side == Side::Sell`: taker sells `amount` base tokens matching against Bids.
    pub fn execute_market_order(env: Env, taker: Address, taker_side: Side, amount: i128) -> i128 {
        taker.require_auth();

        if amount <= 0 {
            panic!("{:?}", OrderBookError::InvalidAmount);
        }

        let config: OrderBookConfig = env
            .storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::NotInitialized));

        let contract_addr = env.current_contract_address();
        let mut remaining = amount;
        let mut total_filled = 0i128;

        match taker_side {
            Side::Buy => {
                // Taker wants to buy base tokens by matching against resting Asks
                let asks: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Asks)
                    .unwrap_or_else(|| Vec::new(&env));
                let mut remaining_asks = Vec::new(&env);

                for i in 0..asks.len() {
                    let ask_id = asks.get(i).unwrap();
                    let mut ask: Order = env
                        .storage()
                        .persistent()
                        .get(&DataKey::Order(ask_id))
                        .unwrap();

                    if remaining <= 0 {
                        remaining_asks.push_back(ask_id);
                        continue;
                    }

                    let unfilled_ask = ask
                        .amount
                        .checked_sub(ask.filled)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

                    let fill_amount = if remaining < unfilled_ask {
                        remaining
                    } else {
                        unfilled_ask
                    };

                    let quote_cost = calculate_quote_amount(fill_amount, ask.price)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

                    // Taker transfers quote tokens directly to maker
                    TokenClient::new(&env, &config.quote_token).transfer(
                        &taker,
                        &ask.maker,
                        &quote_cost,
                    );

                    // Contract delivers escrowed base tokens to taker
                    TokenClient::new(&env, &config.base_token).transfer(
                        &contract_addr,
                        &taker,
                        &fill_amount,
                    );

                    ask.filled = ask
                        .filled
                        .checked_add(fill_amount)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
                    remaining = remaining
                        .checked_sub(fill_amount)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
                    total_filled = total_filled
                        .checked_add(fill_amount)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

                    env.events().publish(
                        (symbol_short!("trade"), taker.clone()),
                        (ask_id, fill_amount, ask.price, quote_cost),
                    );

                    if ask.filled >= ask.amount {
                        // Order fully filled: delete from persistent storage!
                        env.storage().persistent().remove(&DataKey::Order(ask_id));
                    } else {
                        // Partially filled: update in persistent storage
                        env.storage()
                            .persistent()
                            .set(&DataKey::Order(ask_id), &ask);
                        remaining_asks.push_back(ask_id);
                    }
                }

                env.storage()
                    .persistent()
                    .set(&DataKey::Asks, &remaining_asks);
            }
            Side::Sell => {
                // Taker wants to sell base tokens by matching against resting Bids
                let bids: Vec<u64> = env
                    .storage()
                    .persistent()
                    .get(&DataKey::Bids)
                    .unwrap_or_else(|| Vec::new(&env));
                let mut remaining_bids = Vec::new(&env);

                for i in 0..bids.len() {
                    let bid_id = bids.get(i).unwrap();
                    let mut bid: Order = env
                        .storage()
                        .persistent()
                        .get(&DataKey::Order(bid_id))
                        .unwrap();

                    if remaining <= 0 {
                        remaining_bids.push_back(bid_id);
                        continue;
                    }

                    let unfilled_bid = bid
                        .amount
                        .checked_sub(bid.filled)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

                    let fill_amount = if remaining < unfilled_bid {
                        remaining
                    } else {
                        unfilled_bid
                    };

                    let quote_payment = calculate_quote_amount(fill_amount, bid.price)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

                    // Taker transfers base tokens directly to maker
                    TokenClient::new(&env, &config.base_token).transfer(
                        &taker,
                        &bid.maker,
                        &fill_amount,
                    );

                    // Contract delivers escrowed quote tokens to taker
                    TokenClient::new(&env, &config.quote_token).transfer(
                        &contract_addr,
                        &taker,
                        &quote_payment,
                    );

                    bid.filled = bid
                        .filled
                        .checked_add(fill_amount)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
                    remaining = remaining
                        .checked_sub(fill_amount)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));
                    total_filled = total_filled
                        .checked_add(fill_amount)
                        .unwrap_or_else(|| panic!("{:?}", OrderBookError::ArithmeticError));

                    env.events().publish(
                        (symbol_short!("trade"), taker.clone()),
                        (bid_id, fill_amount, bid.price, quote_payment),
                    );

                    if bid.filled >= bid.amount {
                        // Order fully filled: delete from persistent storage!
                        env.storage().persistent().remove(&DataKey::Order(bid_id));
                    } else {
                        // Partially filled: update in persistent storage
                        env.storage()
                            .persistent()
                            .set(&DataKey::Order(bid_id), &bid);
                        remaining_bids.push_back(bid_id);
                    }
                }

                env.storage()
                    .persistent()
                    .set(&DataKey::Bids, &remaining_bids);
            }
        }

        total_filled
    }

    /// Retrieve an order by its ID. Returns None if the order was cancelled or fully filled.
    pub fn get_order(env: Env, order_id: u64) -> Option<Order> {
        env.storage().persistent().get(&DataKey::Order(order_id))
    }

    /// Retrieve sorted list of active bid IDs.
    pub fn get_bids(env: Env) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::Bids)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Retrieve sorted list of active ask IDs.
    pub fn get_asks(env: Env) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::Asks)
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Get total depth (count of open orders) on a given side.
    pub fn get_book_depth(env: Env, side: Side) -> u32 {
        match side {
            Side::Buy => Self::get_bids(env).len(),
            Side::Sell => Self::get_asks(env).len(),
        }
    }

    /// Retrieve contract configuration.
    pub fn get_config(env: Env) -> OrderBookConfig {
        env.storage()
            .persistent()
            .get(&DataKey::Config)
            .unwrap_or_else(|| panic!("{:?}", OrderBookError::NotInitialized))
    }
}
