//! A Dutch (descending price) and batch (uniform clearing price) auction.
//!
//! Both kinds sell units of an `asset` token for a `payment` token, and both
//! escrow everything up front: the seller's units move into the contract when
//! the auction is created, and a bidder's payment moves in when the bid is
//! placed. Settlement is permissionless and pays out from that escrow, so a
//! bid can never be settled against funds the bidder no longer has.
//!
//! # Dutch auction
//!
//! The whole lot of `lot_size` units is sold to the first bidder willing to
//! pay the current asking price. The price falls linearly from `start_price`
//! at `start_time` to `floor_price` at `start_time + duration`, in steps of
//! `step_seconds` (`1` for a continuous decline). The drop is rounded down,
//! so the asking price is rounded up: it never undercuts the schedule and
//! never falls below the floor. A bid names the most the bidder will pay; the
//! contract escrows the asking price at that moment, not the bidder's maximum.
//! Bids are accepted while `start_time <= now <= start_time + duration`.
//!
//! # Batch auction
//!
//! `supply` units are offered at no less than `reserve_price` per unit. Each
//! bidder places at most one bid of `quantity` units at a per-unit
//! `max_price`, escrowing `quantity × max_price`. Bids are accepted while
//! `start_time <= now < end_time`, and settlement runs once `now >= end_time`.
//!
//! Settlement ranks the bids by `max_price` descending, then by submission
//! order (earlier first), and fills them greedily from the supply:
//!
//! * **Undersubscribed** (total demand `<=` supply): every bid is filled in
//!   full and the clearing price is the reserve price. Unsold units return
//!   to the seller.
//! * **Oversubscribed** (total demand `>` supply): bids are filled in rank
//!   order until the supply runs out. The clearing price is the `max_price`
//!   of the *marginal* bid — the last bid to receive any units — which may be
//!   filled only in part. Bids ranked after it receive nothing. Among bids at
//!   the same price the earlier one is filled first, so ties are resolved
//!   deterministically by time priority.
//!
//! Every winner pays `allocation × clearing_price` and is refunded
//! `escrow − payment`; a bid that wins nothing is refunded in full. Because
//! every filled bid ranks at or above the marginal bid,
//! `clearing_price <= max_price` for every winner, so no winner ever pays
//! more than their maximum bid and no refund is negative.
//!
//! Batch payouts are pull-based. `settle` discovers the clearing price and
//! pays the seller `units_sold × clearing_price` plus any unsold units; each
//! bid's allocation and refund are then released by `claim(id, bidder)`,
//! which recomputes the bid's fill from the ranked book. Pushing every payout
//! from `settle` would emit two token transfer events per bidder, and with
//! 50 bidders that is more contract-event data than one Mainnet transaction
//! may carry (16 KiB).

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, token, Address, Env, Vec,
};

/// The most bids one batch auction accepts, bounding the cost of settlement.
pub const MAX_BATCH_BIDS: u32 = 64;

/// Errors the auction raises.
///
/// Codes start at 101 so they cannot be confused with the Stellar Asset
/// Contract's own error codes (1–13), which propagate unchanged through the
/// escrow transfers: an insufficient balance surfaces as `Error(Contract, #10)`
/// from the token, not as an auction error.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum AuctionError {
    AuctionNotFound = 101,
    InvalidConfig = 102,
    NotStarted = 103,
    BiddingClosed = 104,
    BiddingOpen = 105,
    AlreadySettled = 106,
    BidBelowPrice = 107,
    InvalidQuantity = 108,
    DuplicateBid = 109,
    LotAlreadySold = 110,
    TooManyBids = 111,
    ArithmeticOverflow = 112,
    WrongAuctionKind = 113,
    SellerCannotBid = 114,
    /// Settlement accounting would not return every escrowed token. Never
    /// raised by a correct build; it guards the invariant.
    EscrowMismatch = 115,
    /// `get_bid` for an address that never bid on the auction.
    BidNotFound = 116,
    /// `claim` before the auction is settled.
    NotSettled = 117,
    /// A second `claim` for the same bid.
    AlreadyClaimed = 118,
}

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuctionKind {
    Dutch,
    Batch,
}

/// Parameters of a Dutch auction.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DutchConfig {
    pub asset: Address,
    pub payment: Address,
    /// Units of `asset` sold together as one lot.
    pub lot_size: i128,
    /// Asking price for the whole lot at `start_time`.
    pub start_price: i128,
    /// Asking price for the whole lot from `start_time + duration` on.
    pub floor_price: i128,
    pub start_time: u64,
    /// Seconds over which the price declines; must be a multiple of
    /// `step_seconds`.
    pub duration: u64,
    /// The price changes once every `step_seconds`.
    pub step_seconds: u64,
}

/// Parameters of a batch auction.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchConfig {
    pub asset: Address,
    pub payment: Address,
    /// Units of `asset` for sale.
    pub supply: i128,
    /// Minimum per-unit price.
    pub reserve_price: i128,
    pub start_time: u64,
    pub end_time: u64,
}

/// An auction's configuration and state.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Auction {
    pub kind: AuctionKind,
    pub seller: Address,
    pub asset: Address,
    pub payment: Address,
    /// Dutch: lot size. Batch: units for sale.
    pub supply: i128,
    /// Dutch: opening lot price. Batch: reserve price.
    pub start_price: i128,
    /// Dutch: floor lot price. Batch: reserve price.
    pub floor_price: i128,
    pub start_time: u64,
    pub end_time: u64,
    /// Dutch: price step in seconds. Batch: unused (0).
    pub step_seconds: u64,
    pub bid_count: u32,
    /// Batch: units requested across all bids.
    pub total_demand: i128,
    /// Payment tokens currently escrowed for this auction.
    pub total_escrow: i128,
    pub settled: bool,
    /// Dutch: the buyer, once a bid is accepted.
    pub winner: Option<Address>,
    /// Dutch: what the buyer paid for the lot. Batch: the per-unit clearing
    /// price, set at settlement (`None` when nobody bid).
    pub clearing_price: Option<i128>,
    /// Units delivered to buyers at settlement.
    pub units_sold: i128,
    /// Payment tokens delivered to the seller at settlement.
    pub proceeds: i128,
}

/// A batch auction bid and, after settlement, its outcome.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bid {
    pub bidder: Address,
    pub quantity: i128,
    pub max_price: i128,
    /// `quantity × max_price`, held by the contract until settlement.
    pub escrow: i128,
    /// Submission order within the auction, from 0.
    pub sequence: u32,
    /// Units won. Zero until the auction is settled.
    pub allocation: i128,
    /// `allocation × clearing_price`. Zero until the auction is settled.
    pub payment: i128,
    /// `escrow − payment`. Zero until the auction is settled.
    pub refund: i128,
    /// Whether the allocation and refund have been paid out by `claim`.
    pub claimed: bool,
}

/// One entry of a batch auction's ranked order book.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookEntry {
    pub bidder: Address,
    pub quantity: i128,
    pub max_price: i128,
    pub sequence: u32,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    NextId,
    Auction(u64),
    Book(u64),
    Bid(u64, Address),
}

#[contractevent(topics = ["auction", "created"])]
pub struct AuctionCreated {
    pub id: u64,
    pub kind: AuctionKind,
    pub seller: Address,
    pub supply: i128,
}

#[contractevent(topics = ["auction", "dutch_bid"])]
pub struct DutchBidAccepted {
    pub id: u64,
    pub bidder: Address,
    pub price: i128,
}

#[contractevent(topics = ["auction", "batch_bid"])]
pub struct BatchBidPlaced {
    pub id: u64,
    pub bidder: Address,
    pub quantity: i128,
    pub max_price: i128,
    pub escrow: i128,
}

#[contractevent(topics = ["auction", "claimed"])]
pub struct BidClaimed {
    pub id: u64,
    pub bidder: Address,
    pub allocation: i128,
    pub payment: i128,
    pub refund: i128,
}

#[contractevent(topics = ["auction", "settled"])]
pub struct AuctionSettled {
    pub id: u64,
    /// 0 when the auction closed without a sale.
    pub clearing_price: i128,
    pub units_sold: i128,
    pub proceeds: i128,
}

type AuctionResult<T> = Result<T, AuctionError>;

fn load_auction(env: &Env, id: u64) -> AuctionResult<Auction> {
    env.storage()
        .persistent()
        .get(&DataKey::Auction(id))
        .ok_or(AuctionError::AuctionNotFound)
}

fn save_auction(env: &Env, id: u64, auction: &Auction) {
    env.storage()
        .persistent()
        .set(&DataKey::Auction(id), auction);
}

fn load_book(env: &Env, id: u64) -> Vec<BookEntry> {
    env.storage()
        .persistent()
        .get(&DataKey::Book(id))
        .unwrap_or_else(|| Vec::new(env))
}

fn allocate_id(env: &Env) -> AuctionResult<u64> {
    let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
    let next = id.checked_add(1).ok_or(AuctionError::ArithmeticOverflow)?;
    env.storage().instance().set(&DataKey::NextId, &next);
    Ok(id)
}

fn mul(a: i128, b: i128) -> AuctionResult<i128> {
    a.checked_mul(b).ok_or(AuctionError::ArithmeticOverflow)
}

fn add(a: i128, b: i128) -> AuctionResult<i128> {
    a.checked_add(b).ok_or(AuctionError::ArithmeticOverflow)
}

fn sub(a: i128, b: i128) -> AuctionResult<i128> {
    a.checked_sub(b).ok_or(AuctionError::ArithmeticOverflow)
}

fn expect_kind(auction: &Auction, kind: AuctionKind) -> AuctionResult<()> {
    if auction.kind != kind {
        return Err(AuctionError::WrongAuctionKind);
    }
    Ok(())
}

/// The Dutch asking price for the whole lot at `now`.
///
/// `drop = (start_price − floor_price) × stepped_elapsed / duration`, rounded
/// down, so the price is rounded up. Creation checks that
/// `(start_price − floor_price) × duration` fits in an `i128`, so this cannot
/// overflow for any `now`.
fn dutch_price(auction: &Auction, now: u64) -> AuctionResult<i128> {
    let duration = auction.end_time - auction.start_time;
    let elapsed = now.saturating_sub(auction.start_time).min(duration);
    let stepped = elapsed - elapsed % auction.step_seconds;
    let spread = auction.start_price - auction.floor_price;
    let drop = mul(spread, stepped as i128)? / duration as i128;
    Ok(auction.start_price - drop)
}

/// The clearing price of a batch auction's ranked `book`, or `None` when
/// nobody bid. Undersubscribed (demand `<=` supply) clears at the reserve;
/// oversubscribed clears at the `max_price` of the marginal bid, the last one
/// the greedy fill reaches.
fn batch_clearing_price(auction: &Auction, book: &Vec<BookEntry>) -> Option<i128> {
    if book.is_empty() {
        return None;
    }
    if auction.total_demand <= auction.supply {
        return Some(auction.floor_price);
    }
    let mut remaining = auction.supply;
    let mut marginal = None;
    for entry in book.iter() {
        if remaining == 0 {
            break;
        }
        remaining -= entry.quantity.min(remaining);
        marginal = Some(entry.max_price);
    }
    marginal
}

/// Fills in `bid`'s allocation, payment and refund for a settled auction:
/// the greedy fill gives it `min(quantity, supply − units ranked ahead)`.
fn settle_outcome(auction: &Auction, book: &Vec<BookEntry>, bid: &mut Bid) -> AuctionResult<()> {
    let price = auction.clearing_price.unwrap_or(0);
    let mut remaining = auction.supply;
    for entry in book.iter() {
        if entry.bidder == bid.bidder {
            break;
        }
        remaining -= entry.quantity.min(remaining);
    }
    bid.allocation = bid.quantity.min(remaining);
    bid.payment = mul(bid.allocation, price)?;
    bid.refund = sub(bid.escrow, bid.payment)?;
    if bid.refund < 0 {
        // Unreachable by construction (allocation <= quantity and
        // price <= max_price); refuse rather than over-charge.
        return Err(AuctionError::EscrowMismatch);
    }
    Ok(())
}

fn escrow_from(env: &Env, token: &Address, from: &Address, amount: i128) {
    token::Client::new(env, token).transfer(from, env.current_contract_address(), &amount);
}

fn pay_out(env: &Env, token: &Address, to: &Address, amount: i128) {
    if amount > 0 {
        token::Client::new(env, token).transfer(&env.current_contract_address(), to, &amount);
    }
}

#[contract]
pub struct AuctionHouse;

#[contractimpl]
impl AuctionHouse {
    /// Opens a Dutch auction and escrows the seller's lot. Returns its id.
    pub fn create_dutch(
        env: Env,
        seller: Address,
        config: DutchConfig,
    ) -> Result<u64, AuctionError> {
        seller.require_auth();
        if config.lot_size <= 0
            || config.floor_price <= 0
            || config.start_price < config.floor_price
            || config.duration == 0
            || config.step_seconds == 0
            || config.step_seconds > config.duration
            || !config.duration.is_multiple_of(config.step_seconds)
        {
            return Err(AuctionError::InvalidConfig);
        }
        let end_time = config
            .start_time
            .checked_add(config.duration)
            .ok_or(AuctionError::ArithmeticOverflow)?;
        // Proves dutch_price can never overflow.
        mul(
            config.start_price - config.floor_price,
            config.duration as i128,
        )?;

        escrow_from(&env, &config.asset, &seller, config.lot_size);

        let id = allocate_id(&env)?;
        let auction = Auction {
            kind: AuctionKind::Dutch,
            seller: seller.clone(),
            asset: config.asset,
            payment: config.payment,
            supply: config.lot_size,
            start_price: config.start_price,
            floor_price: config.floor_price,
            start_time: config.start_time,
            end_time,
            step_seconds: config.step_seconds,
            bid_count: 0,
            total_demand: 0,
            total_escrow: 0,
            settled: false,
            winner: None,
            clearing_price: None,
            units_sold: 0,
            proceeds: 0,
        };
        save_auction(&env, id, &auction);
        AuctionCreated {
            id,
            kind: AuctionKind::Dutch,
            seller,
            supply: auction.supply,
        }
        .publish(&env);
        Ok(id)
    }

    /// Opens a batch auction and escrows the seller's supply. Returns its id.
    pub fn create_batch(
        env: Env,
        seller: Address,
        config: BatchConfig,
    ) -> Result<u64, AuctionError> {
        seller.require_auth();
        if config.supply <= 0 || config.reserve_price <= 0 || config.end_time <= config.start_time {
            return Err(AuctionError::InvalidConfig);
        }

        escrow_from(&env, &config.asset, &seller, config.supply);

        let id = allocate_id(&env)?;
        let auction = Auction {
            kind: AuctionKind::Batch,
            seller: seller.clone(),
            asset: config.asset,
            payment: config.payment,
            supply: config.supply,
            start_price: config.reserve_price,
            floor_price: config.reserve_price,
            start_time: config.start_time,
            end_time: config.end_time,
            step_seconds: 0,
            bid_count: 0,
            total_demand: 0,
            total_escrow: 0,
            settled: false,
            winner: None,
            clearing_price: None,
            units_sold: 0,
            proceeds: 0,
        };
        save_auction(&env, id, &auction);
        AuctionCreated {
            id,
            kind: AuctionKind::Batch,
            seller,
            supply: auction.supply,
        }
        .publish(&env);
        Ok(id)
    }

    /// Buys a Dutch lot at the current asking price, provided it is no more
    /// than `max_price`. Escrows the asking price and returns it.
    pub fn bid_dutch(
        env: Env,
        id: u64,
        bidder: Address,
        max_price: i128,
    ) -> Result<i128, AuctionError> {
        bidder.require_auth();
        let mut auction = load_auction(&env, id)?;
        expect_kind(&auction, AuctionKind::Dutch)?;
        if auction.settled {
            return Err(AuctionError::AlreadySettled);
        }
        if auction.winner.is_some() {
            return Err(AuctionError::LotAlreadySold);
        }
        if bidder == auction.seller {
            return Err(AuctionError::SellerCannotBid);
        }
        let now = env.ledger().timestamp();
        if now < auction.start_time {
            return Err(AuctionError::NotStarted);
        }
        if now > auction.end_time {
            return Err(AuctionError::BiddingClosed);
        }
        let price = dutch_price(&auction, now)?;
        if max_price < price {
            return Err(AuctionError::BidBelowPrice);
        }

        escrow_from(&env, &auction.payment, &bidder, price);

        auction.winner = Some(bidder.clone());
        auction.clearing_price = Some(price);
        auction.bid_count = 1;
        auction.total_escrow = price;
        save_auction(&env, id, &auction);
        DutchBidAccepted { id, bidder, price }.publish(&env);
        Ok(price)
    }

    /// Places a batch bid of `quantity` units at up to `max_price` per unit,
    /// escrowing `quantity × max_price`.
    pub fn bid_batch(
        env: Env,
        id: u64,
        bidder: Address,
        quantity: i128,
        max_price: i128,
    ) -> Result<(), AuctionError> {
        bidder.require_auth();
        let mut auction = load_auction(&env, id)?;
        expect_kind(&auction, AuctionKind::Batch)?;
        if auction.settled {
            return Err(AuctionError::AlreadySettled);
        }
        if bidder == auction.seller {
            return Err(AuctionError::SellerCannotBid);
        }
        let now = env.ledger().timestamp();
        if now < auction.start_time {
            return Err(AuctionError::NotStarted);
        }
        if now >= auction.end_time {
            return Err(AuctionError::BiddingClosed);
        }
        if quantity <= 0 {
            return Err(AuctionError::InvalidQuantity);
        }
        if max_price < auction.floor_price {
            return Err(AuctionError::BidBelowPrice);
        }
        let bid_key = DataKey::Bid(id, bidder.clone());
        if env.storage().persistent().has(&bid_key) {
            return Err(AuctionError::DuplicateBid);
        }
        if auction.bid_count >= MAX_BATCH_BIDS {
            return Err(AuctionError::TooManyBids);
        }
        let escrow = mul(quantity, max_price)?;
        auction.total_demand = add(auction.total_demand, quantity)?;
        auction.total_escrow = add(auction.total_escrow, escrow)?;

        escrow_from(&env, &auction.payment, &bidder, escrow);

        let sequence = auction.bid_count;
        auction.bid_count += 1;

        // Keep the book ranked: price descending, then submission order. A
        // new bid goes after every bid at its price or higher.
        let mut book = load_book(&env, id);
        let mut position = book.len();
        for (index, entry) in book.iter().enumerate() {
            if entry.max_price < max_price {
                position = index as u32;
                break;
            }
        }
        book.insert(
            position,
            BookEntry {
                bidder: bidder.clone(),
                quantity,
                max_price,
                sequence,
            },
        );

        env.storage().persistent().set(
            &bid_key,
            &Bid {
                bidder: bidder.clone(),
                quantity,
                max_price,
                escrow,
                sequence,
                allocation: 0,
                payment: 0,
                refund: 0,
                claimed: false,
            },
        );
        env.storage().persistent().set(&DataKey::Book(id), &book);
        save_auction(&env, id, &auction);
        BatchBidPlaced {
            id,
            bidder,
            quantity,
            max_price,
            escrow,
        }
        .publish(&env);
        Ok(())
    }

    /// Settles an auction, paying out from escrow. Anyone may call it, once.
    ///
    /// Dutch: delivers the lot to the buyer and the price to the seller, or,
    /// after the bidding window closed without a buyer, returns the lot.
    /// Batch: after `end_time`, discovers the clearing price, delivers each
    /// allocation, refunds excess escrow and pays the seller.
    pub fn settle(env: Env, id: u64) -> Result<Auction, AuctionError> {
        let mut auction = load_auction(&env, id)?;
        if auction.settled {
            return Err(AuctionError::AlreadySettled);
        }
        let now = env.ledger().timestamp();
        match auction.kind {
            AuctionKind::Dutch => Self::settle_dutch(&env, &mut auction, now)?,
            AuctionKind::Batch => Self::settle_batch(&env, id, &mut auction, now)?,
        }
        auction.settled = true;
        save_auction(&env, id, &auction);
        AuctionSettled {
            id,
            clearing_price: auction.clearing_price.unwrap_or(0),
            units_sold: auction.units_sold,
            proceeds: auction.proceeds,
        }
        .publish(&env);
        Ok(auction)
    }

    /// The auction's configuration and state.
    pub fn auction(env: Env, id: u64) -> Result<Auction, AuctionError> {
        load_auction(&env, id)
    }

    /// The Dutch asking price for the lot at the current ledger time.
    pub fn current_price(env: Env, id: u64) -> Result<i128, AuctionError> {
        let auction = load_auction(&env, id)?;
        expect_kind(&auction, AuctionKind::Dutch)?;
        dutch_price(&auction, env.ledger().timestamp())
    }

    /// A batch bid and, once the auction is settled, its allocation,
    /// payment and refund (whether or not they have been claimed yet).
    pub fn get_bid(env: Env, id: u64, bidder: Address) -> Result<Bid, AuctionError> {
        let auction = load_auction(&env, id)?;
        let mut bid: Bid = env
            .storage()
            .persistent()
            .get(&DataKey::Bid(id, bidder))
            .ok_or(AuctionError::BidNotFound)?;
        if auction.settled && !bid.claimed {
            settle_outcome(&auction, &load_book(&env, id), &mut bid)?;
        }
        Ok(bid)
    }

    /// Pays a settled batch bid out of escrow: the allocated units and the
    /// refund of `escrow − allocation × clearing_price` go to the bidder.
    /// Anyone may call it, since the funds can only go to the bidder; each
    /// bid is claimed once.
    pub fn claim(env: Env, id: u64, bidder: Address) -> Result<Bid, AuctionError> {
        let mut auction = load_auction(&env, id)?;
        expect_kind(&auction, AuctionKind::Batch)?;
        if !auction.settled {
            return Err(AuctionError::NotSettled);
        }
        let key = DataKey::Bid(id, bidder.clone());
        let mut bid: Bid = env
            .storage()
            .persistent()
            .get(&key)
            .ok_or(AuctionError::BidNotFound)?;
        if bid.claimed {
            return Err(AuctionError::AlreadyClaimed);
        }
        settle_outcome(&auction, &load_book(&env, id), &mut bid)?;
        bid.claimed = true;
        auction.total_escrow = sub(auction.total_escrow, bid.refund)?;

        pay_out(&env, &auction.asset, &bidder, bid.allocation);
        pay_out(&env, &auction.payment, &bidder, bid.refund);
        env.storage().persistent().set(&key, &bid);
        save_auction(&env, id, &auction);
        BidClaimed {
            id,
            bidder,
            allocation: bid.allocation,
            payment: bid.payment,
            refund: bid.refund,
        }
        .publish(&env);
        Ok(bid)
    }

    /// The batch order book in settlement rank order.
    pub fn book(env: Env, id: u64) -> Result<Vec<BookEntry>, AuctionError> {
        load_auction(&env, id)?;
        Ok(load_book(&env, id))
    }
}

impl AuctionHouse {
    fn settle_dutch(env: &Env, auction: &mut Auction, now: u64) -> AuctionResult<()> {
        match auction.winner.clone() {
            Some(winner) => {
                let price = auction.clearing_price.unwrap_or(0);
                pay_out(env, &auction.asset, &winner, auction.supply);
                pay_out(env, &auction.payment, &auction.seller, price);
                auction.units_sold = auction.supply;
                auction.proceeds = price;
                auction.total_escrow = 0;
            }
            None => {
                if now <= auction.end_time {
                    return Err(AuctionError::BiddingOpen);
                }
                pay_out(env, &auction.asset, &auction.seller, auction.supply);
            }
        }
        Ok(())
    }

    fn settle_batch(env: &Env, id: u64, auction: &mut Auction, now: u64) -> AuctionResult<()> {
        if now < auction.end_time {
            return Err(AuctionError::BiddingOpen);
        }
        let book = load_book(env, id);
        let clearing = batch_clearing_price(auction, &book);
        let units_sold = auction.supply.min(auction.total_demand);
        // A uniform price: every unit sold pays the same clearing price.
        let proceeds = mul(units_sold, clearing.unwrap_or(0))?;
        if proceeds > auction.total_escrow {
            return Err(AuctionError::EscrowMismatch);
        }

        pay_out(env, &auction.payment, &auction.seller, proceeds);
        pay_out(
            env,
            &auction.asset,
            &auction.seller,
            auction.supply - units_sold,
        );
        auction.clearing_price = clearing;
        auction.units_sold = units_sold;
        auction.proceeds = proceeds;
        // What is left in escrow is exactly the bidders' refunds, released
        // one bid at a time by `claim`.
        auction.total_escrow -= proceeds;
        Ok(())
    }
}
