#![no_std]

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, token, Address, Env, Map, Symbol,
};

/// The two outcomes a binary market can resolve to.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The market resolves true: the oracle price reached the strike.
    Yes,
    /// The market resolves false: the oracle price stayed below the strike.
    No,
}

/// Where a market is in its life.
///
/// A market is `Open` from creation until the oracle resolves it, and
/// `Resolved` from then on. Stakes are locked for the whole of `Open`, and
/// winning shares are redeemable only after the dispute window in `Resolved`.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarketStatus {
    Open,
    Resolved,
}

/// One binary market and every position in it.
///
/// The whole record lives under one persistent map entry, so a purchase reads
/// and rewrites one key rather than several. `yes_holders` and `no_holders`
/// are the outstanding positions; a winning holder's entry is removed when
/// they redeem, which is what makes a second claim impossible without a
/// separate "already claimed" set.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct Market {
    /// The SEP-41 token all collateral is denominated in.
    pub token: Address,
    /// The only address allowed to report the resolution price.
    pub oracle: Address,
    /// The price at or above which the market resolves `Yes`.
    pub threshold: i128,
    /// Unix timestamp after which the market stops accepting purchases.
    pub close_time: u64,
    /// Seconds between resolution and the first permitted redemption.
    pub dispute_delay: u64,
    /// Current phase.
    pub status: MarketStatus,
    /// The winning side. Meaningful once `status` is `Resolved`; while the
    /// market is open the field holds its initial `No` and is ignored.
    pub outcome: Outcome,
    /// Unix timestamp the dispute window closes at; `0` while open.
    pub redeemable_at: u64,
    /// Every token of collateral the market holds.
    pub pool: i128,
    /// Outstanding YES shares.
    pub yes_shares: i128,
    /// Outstanding NO shares.
    pub no_shares: i128,
    /// Winning shares nobody has redeemed yet.
    pub winning_shares_remaining: i128,
    /// Collateral nobody has been paid yet.
    pub collateral_remaining: i128,
    /// Outstanding YES positions, by holder.
    pub yes_holders: Map<Address, i128>,
    /// Outstanding NO positions, by holder.
    pub no_holders: Map<Address, i128>,
}

/// A market was created.
#[contractevent(topics = ["market", "created"])]
#[derive(Clone)]
pub struct MarketCreated {
    pub id: u64,
    pub token: Address,
    pub oracle: Address,
    pub threshold: i128,
    pub close_time: u64,
    pub dispute_delay: u64,
}

/// A trader bought outcome shares.
#[contractevent(topics = ["market", "bought"])]
#[derive(Clone)]
pub struct SharesBought {
    pub id: u64,
    pub buyer: Address,
    pub outcome: Outcome,
    pub amount: i128,
}

/// The oracle resolved the market.
#[contractevent(topics = ["market", "resolved"])]
#[derive(Clone)]
pub struct MarketResolved {
    pub id: u64,
    pub outcome: Outcome,
    pub pool: i128,
    pub redeemable_at: u64,
}

/// A winning holder redeemed their shares.
#[contractevent(topics = ["market", "redeemed"])]
#[derive(Clone)]
pub struct SharesRedeemed {
    pub id: u64,
    pub holder: Address,
    pub shares: i128,
    pub payout: i128,
}

fn markets_key(env: &Env) -> Symbol {
    Symbol::new(env, "markets")
}

fn next_id_key(env: &Env) -> Symbol {
    Symbol::new(env, "next_id")
}

fn markets(env: &Env) -> Map<u64, Market> {
    env.storage()
        .persistent()
        .get(&markets_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_markets(env: &Env, map: &Map<u64, Market>) {
    env.storage().persistent().set(&markets_key(env), map);
}

fn read_market(env: &Env, id: u64) -> Market {
    markets(env).get(id).expect("market not found")
}

fn store_market(env: &Env, id: u64, market: &Market) {
    let mut map = markets(env);
    map.set(id, market.clone());
    write_markets(env, &map);
}

fn next_id(env: &Env) -> u64 {
    env.storage().instance().get(&next_id_key(env)).unwrap_or(0)
}

fn position(map: &Map<Address, i128>, holder: &Address) -> i128 {
    map.get(holder.clone()).unwrap_or(0)
}

fn winning_position(market: &Market, holder: &Address) -> i128 {
    if market.status != MarketStatus::Resolved {
        return 0;
    }
    match market.outcome {
        Outcome::Yes => position(&market.yes_holders, holder),
        Outcome::No => position(&market.no_holders, holder),
    }
}

/// A binary prediction market with oracle resolution.
///
/// Collateral from both sides is pooled. The oracle resolves the market once
/// after `close_time`; a dispute delay then keeps the winning shares locked
/// before they can be redeemed. Every winner's payout is a pro-rata slice of
/// the whole pool, and the last winner receives the remainder, so redemption
/// distributes exactly the collateral the market holds.
#[contract]
pub struct PredictionMarketContract;

#[contractimpl]
impl PredictionMarketContract {
    /// Opens a market and returns its id.
    ///
    /// `threshold` is the price at or above which the market resolves `Yes`.
    /// `close_time` must be in the future, and `dispute_delay` is how long
    /// redemption stays locked after resolution.
    pub fn create(
        env: Env,
        token: Address,
        oracle: Address,
        threshold: i128,
        close_time: u64,
        dispute_delay: u64,
    ) -> u64 {
        assert!(threshold > 0, "threshold must be positive");
        assert!(
            close_time > env.ledger().timestamp(),
            "close time must be in the future"
        );

        let id = next_id(&env) + 1;
        env.storage().instance().set(&next_id_key(&env), &id);

        let market = Market {
            token: token.clone(),
            oracle: oracle.clone(),
            threshold,
            close_time,
            dispute_delay,
            status: MarketStatus::Open,
            outcome: Outcome::No,
            redeemable_at: 0,
            pool: 0,
            yes_shares: 0,
            no_shares: 0,
            winning_shares_remaining: 0,
            collateral_remaining: 0,
            yes_holders: Map::new(&env),
            no_holders: Map::new(&env),
        };
        store_market(&env, id, &market);

        MarketCreated {
            id,
            token,
            oracle,
            threshold,
            close_time,
            dispute_delay,
        }
        .publish(&env);

        id
    }

    /// Buys `amount` shares of `outcome`, transferring the same amount of
    /// collateral into the pool.
    ///
    /// Refused after `close_time`, so the outcome cannot be bought with
    /// knowledge of how the market is about to resolve.
    pub fn buy(env: Env, id: u64, buyer: Address, outcome: Outcome, amount: i128) {
        buyer.require_auth();
        assert!(amount > 0, "amount must be positive");

        let mut market = read_market(&env, id);
        assert!(
            market.status == MarketStatus::Open,
            "market is no longer open"
        );
        assert!(
            env.ledger().timestamp() < market.close_time,
            "market has closed"
        );

        token::Client::new(&env, &market.token).transfer(
            &buyer,
            env.current_contract_address(),
            &amount,
        );

        match outcome {
            Outcome::Yes => {
                let held = position(&market.yes_holders, &buyer) + amount;
                market.yes_holders.set(buyer.clone(), held);
                market.yes_shares += amount;
            }
            Outcome::No => {
                let held = position(&market.no_holders, &buyer) + amount;
                market.no_holders.set(buyer.clone(), held);
                market.no_shares += amount;
            }
        }
        market.pool += amount;
        store_market(&env, id, &market);

        SharesBought {
            id,
            buyer,
            outcome,
            amount,
        }
        .publish(&env);
    }

    /// Resolves the market from an oracle price report.
    ///
    /// Only the market's oracle may resolve, only once, and only after the
    /// market has closed. Both sides must hold shares: a one-sided market has
    /// no losing collateral to pay the winners with, so resolving it would
    /// strand the pool and is refused.
    ///
    /// A price at or above the threshold resolves `Yes`; anything below it
    /// resolves `No`. Redemption unlocks `dispute_delay` seconds later.
    pub fn resolve(env: Env, id: u64, oracle: Address, price: i128) {
        oracle.require_auth();

        let mut market = read_market(&env, id);
        assert!(
            market.status == MarketStatus::Open,
            "market has already been resolved"
        );
        assert!(oracle == market.oracle, "caller is not the market oracle");
        assert!(
            env.ledger().timestamp() >= market.close_time,
            "market has not closed yet"
        );
        assert!(price > 0, "oracle price must be positive");
        assert!(
            market.yes_shares > 0 && market.no_shares > 0,
            "both outcomes need collateral before resolution"
        );

        let outcome = if price >= market.threshold {
            Outcome::Yes
        } else {
            Outcome::No
        };
        let winning_shares = match outcome {
            Outcome::Yes => market.yes_shares,
            Outcome::No => market.no_shares,
        };

        market.status = MarketStatus::Resolved;
        market.outcome = outcome;
        market.redeemable_at = env.ledger().timestamp() + market.dispute_delay;
        market.winning_shares_remaining = winning_shares;
        market.collateral_remaining = market.pool;
        store_market(&env, id, &market);

        MarketResolved {
            id,
            outcome,
            pool: market.pool,
            redeemable_at: market.redeemable_at,
        }
        .publish(&env);
    }

    /// Redeems a winning position after the dispute window, returning the
    /// payout.
    ///
    /// The payout is the holder's share of the remaining pool. Integer
    /// division can leave a remainder, so the final winner receives whatever
    /// collateral is left; across all redemptions the payouts sum to exactly
    /// the pool. A holder with no shares on the winning side has nothing to
    /// redeem and is refused rather than paid zero.
    pub fn claim(env: Env, id: u64, holder: Address) -> i128 {
        holder.require_auth();

        let mut market = read_market(&env, id);
        assert!(
            market.status == MarketStatus::Resolved,
            "market has not been resolved"
        );
        assert!(
            env.ledger().timestamp() >= market.redeemable_at,
            "dispute window is still open"
        );

        let shares = winning_position(&market, &holder);
        assert!(shares > 0, "holder has no winning shares");

        let remaining_shares = market.winning_shares_remaining;
        assert!(remaining_shares > 0, "winning pool is already settled");

        let payout = if shares >= remaining_shares {
            market.collateral_remaining
        } else {
            shares * market.collateral_remaining / remaining_shares
        };

        match market.outcome {
            Outcome::Yes => market.yes_holders.remove(holder.clone()),
            Outcome::No => market.no_holders.remove(holder.clone()),
        };

        market.winning_shares_remaining -= shares;
        market.collateral_remaining -= payout;
        store_market(&env, id, &market);

        token::Client::new(&env, &market.token).transfer(
            &env.current_contract_address(),
            &holder,
            &payout,
        );

        SharesRedeemed {
            id,
            holder,
            shares,
            payout,
        }
        .publish(&env);

        payout
    }

    /// Everything recorded about a market.
    pub fn market(env: Env, id: u64) -> Market {
        read_market(&env, id)
    }

    /// The market's current phase.
    pub fn status(env: Env, id: u64) -> MarketStatus {
        read_market(&env, id).status
    }

    /// The winning side, or `None` while the market is open.
    pub fn winning_outcome(env: Env, id: u64) -> Option<Outcome> {
        let market = read_market(&env, id);
        match market.status {
            MarketStatus::Open => None,
            MarketStatus::Resolved => Some(market.outcome),
        }
    }

    /// How much collateral the market holds.
    pub fn pool(env: Env, id: u64) -> i128 {
        read_market(&env, id).pool
    }

    /// The timestamp redemption unlocks at; `0` while the market is open.
    pub fn redeemable_at(env: Env, id: u64) -> u64 {
        read_market(&env, id).redeemable_at
    }

    /// A holder's outstanding shares of one outcome.
    pub fn position(env: Env, id: u64, holder: Address, outcome: Outcome) -> i128 {
        let market = read_market(&env, id);
        match outcome {
            Outcome::Yes => position(&market.yes_holders, &holder),
            Outcome::No => position(&market.no_holders, &holder),
        }
    }

    /// The id the next `create` will assign. Ids start at 1.
    pub fn next_market_id(env: Env) -> u64 {
        next_id(&env) + 1
    }
}
