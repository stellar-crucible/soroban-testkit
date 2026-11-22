#![allow(dead_code)]

//! Shared setup for the auction suites: a `TestContext` from
//! `soroban-testkit-fixtures`, the auction contract, an asset token being
//! sold, a payment token bids are escrowed in, and funded bidders.

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::Address;
use soroban_testkit_example_auction::{
    AuctionHouse, AuctionHouseClient, BatchConfig, Bid, DutchConfig,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::{NetworkProfile, TestContext};

/// The ledger clock every fixture starts at.
pub const T0: u64 = 1_700_000_000;
/// Asset units the seller starts with.
pub const SELLER_ASSET: i128 = 1_000_000;
/// Payment tokens each bidder starts with.
pub const BIDDER_FUNDS: i128 = 1_000_000_000;

/// Dutch defaults: a lot of 100 units, 10_000 falling to 4_000 over 600 s.
pub const LOT: i128 = 100;
pub const START_PRICE: i128 = 10_000;
pub const FLOOR_PRICE: i128 = 4_000;
pub const DECAY: u64 = 600;

/// Batch defaults: 1_000 units, reserve 10 per unit, a one hour window.
pub const SUPPLY: i128 = 1_000;
pub const RESERVE: i128 = 10;
pub const WINDOW: u64 = 3_600;

pub struct House {
    pub ctx: TestContext,
    pub contract_id: Address,
    pub asset: Address,
    pub payment: Address,
    pub seller: Address,
}

impl House {
    /// A deployed auction house with `bidders` funded bidders.
    pub fn new(bidders: usize) -> Self {
        Self::with_profile(bidders, NetworkProfile::Local)
    }

    /// The same deployment under a network profile's resource limits.
    pub fn with_profile(bidders: usize, profile: NetworkProfile) -> Self {
        let mut ctx = TestContextBuilder::new()
            .with_users(bidders)
            .with_network_profile(profile)
            .build();
        ctx.set_timestamp(T0);

        let contract_id = ctx.env.register(AuctionHouse, ());
        let asset = ctx
            .env
            .register_stellar_asset_contract_v2(Address::generate(&ctx.env))
            .address();
        let payment = ctx
            .env
            .register_stellar_asset_contract_v2(Address::generate(&ctx.env))
            .address();
        let seller = ctx.admin.clone();

        let house = Self {
            ctx,
            contract_id,
            asset,
            payment,
            seller,
        };
        StellarAssetClient::new(&house.ctx.env, &house.asset).mint(&house.seller, &SELLER_ASSET);
        for bidder in house.ctx.users.clone() {
            house.fund(&bidder, BIDDER_FUNDS);
        }
        house
    }

    pub fn client(&self) -> AuctionHouseClient<'_> {
        AuctionHouseClient::new(&self.ctx.env, &self.contract_id)
    }

    pub fn bidder(&self, index: usize) -> Address {
        self.ctx.user(index).clone()
    }

    pub fn bidders(&self) -> Vec<Address> {
        self.ctx.users.clone()
    }

    /// A fresh address holding no tokens at all.
    pub fn unfunded(&self) -> Address {
        Address::generate(&self.ctx.env)
    }

    pub fn fund(&self, who: &Address, amount: i128) {
        StellarAssetClient::new(&self.ctx.env, &self.payment).mint(who, &amount);
    }

    pub fn asset_token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.ctx.env, &self.asset)
    }

    pub fn payment_token(&self) -> TokenClient<'_> {
        TokenClient::new(&self.ctx.env, &self.payment)
    }

    pub fn asset_of(&self, who: &Address) -> i128 {
        self.asset_token().balance(who)
    }

    pub fn payment_of(&self, who: &Address) -> i128 {
        self.payment_token().balance(who)
    }

    pub fn now(&self) -> u64 {
        self.ctx.timestamp()
    }

    /// Moves the ledger clock forward through the fixtures crate.
    pub fn advance(&mut self, seconds: u64) -> u64 {
        self.ctx.advance_time(seconds)
    }

    pub fn dutch_config(&self, start: i128, floor: i128, duration: u64, step: u64) -> DutchConfig {
        DutchConfig {
            asset: self.asset.clone(),
            payment: self.payment.clone(),
            lot_size: LOT,
            start_price: start,
            floor_price: floor,
            start_time: self.now(),
            duration,
            step_seconds: step,
        }
    }

    pub fn batch_config(&self, supply: i128, reserve: i128) -> BatchConfig {
        BatchConfig {
            asset: self.asset.clone(),
            payment: self.payment.clone(),
            supply,
            reserve_price: reserve,
            start_time: self.now(),
            end_time: self.now() + WINDOW,
        }
    }

    /// A Dutch auction starting now with the given schedule.
    pub fn dutch_with(&self, start: i128, floor: i128, duration: u64, step: u64) -> u64 {
        self.client().create_dutch(
            &self.seller,
            &self.dutch_config(start, floor, duration, step),
        )
    }

    /// A Dutch auction starting now with the default continuous schedule.
    pub fn dutch(&self) -> u64 {
        self.dutch_with(START_PRICE, FLOOR_PRICE, DECAY, 1)
    }

    /// A batch auction open from now for `WINDOW` seconds.
    pub fn batch_with(&self, supply: i128, reserve: i128) -> u64 {
        self.client()
            .create_batch(&self.seller, &self.batch_config(supply, reserve))
    }

    pub fn batch(&self) -> u64 {
        self.batch_with(SUPPLY, RESERVE)
    }

    /// Places `(quantity, max_price)` bids from bidders 0, 1, 2, ... in order.
    pub fn place_bids(&self, id: u64, bids: &[(i128, i128)]) {
        for (index, (quantity, max_price)) in bids.iter().enumerate() {
            self.client()
                .bid_batch(&id, &self.bidder(index), quantity, max_price);
        }
    }

    /// Closes the bidding window of a batch auction created at the fixture's
    /// current time.
    pub fn close_window(&mut self) {
        self.advance(WINDOW);
    }

    /// Claims every bid placed by bidders `0..count`.
    pub fn claim_all(&self, id: u64, count: usize) -> Vec<Bid> {
        (0..count)
            .map(|index| self.client().claim(&id, &self.bidder(index)))
            .collect()
    }

    pub fn bid_of(&self, id: u64, index: usize) -> Bid {
        self.client().get_bid(&id, &self.bidder(index))
    }
}
