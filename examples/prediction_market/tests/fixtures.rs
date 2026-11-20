//! One crate per file: this suite drives the prediction market through
//! `soroban-testkit-fixtures`, whose job is the fixture every other file
//! assumes — a funded token, a registered market and one identity per role.
//!
//! The lifecycle assertions live here: creation, buying either side,
//! resolution, dispute delay and redemption.

extern crate std;

use soroban_sdk::{testutils::Ledger as _, token::StellarAssetClient, Address, Vec};
use soroban_testkit_example_prediction_market::{
    MarketStatus, Outcome, PredictionMarketContract, PredictionMarketContractClient,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const CLOSE_TIME: u64 = 1_000;
const DISPUTE_DELAY: u64 = 300;

/// A deployed market contract, a funded token, an oracle and four traders.
struct MarketFixture {
    ctx: TestContext,
    contract: Address,
    token: Address,
    oracle: Address,
    traders: Vec<Address>,
}

fn setup() -> MarketFixture {
    let ctx = TestContextBuilder::new().with_users(6).build();

    let contract = ctx.env.register(PredictionMarketContract, ());
    let token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let mut traders = Vec::new(&ctx.env);
    for index in 0..4 {
        let trader = ctx.user(index).clone();
        StellarAssetClient::new(&ctx.env, &token).mint(&trader, &10_000);
        traders.push_back(trader);
    }

    MarketFixture {
        contract,
        token,
        oracle: ctx.user(5).clone(),
        traders,
        ctx,
    }
}

impl MarketFixture {
    fn client(&self) -> PredictionMarketContractClient<'_> {
        PredictionMarketContractClient::new(&self.ctx.env, &self.contract)
    }

    fn token_client(&self) -> soroban_sdk::token::Client<'_> {
        soroban_sdk::token::Client::new(&self.ctx.env, &self.token)
    }

    fn trader(&self, index: u32) -> Address {
        self.traders.get(index).unwrap()
    }

    fn create(&self) -> u64 {
        self.client()
            .create(&self.token, &self.oracle, &100, &CLOSE_TIME, &DISPUTE_DELAY)
    }

    fn buy(&self, id: u64, index: u32, outcome: Outcome, amount: i128) {
        self.client()
            .buy(&id, &self.trader(index), &outcome, &amount);
    }

    /// A two-sided market, closed and resolved to the requested outcome.
    fn resolved(&self, outcome: Outcome) -> u64 {
        let id = self.create();
        self.buy(id, 0, Outcome::Yes, 100);
        self.buy(id, 1, Outcome::No, 50);
        self.ctx.env.ledger().set_timestamp(CLOSE_TIME + 1);
        let price = if outcome == Outcome::Yes { 100 } else { 99 };
        self.client().resolve(&id, &self.oracle, &price);
        id
    }

    fn escrow(&self) -> i128 {
        self.token_client().balance(&self.contract)
    }
}

#[test]
fn create_assigns_incrementing_ids_and_an_open_market() {
    let fixture = setup();
    let client = fixture.client();

    let first = fixture.create();
    let second = fixture.create();

    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(client.next_market_id(), 3);
    assert_eq!(client.status(&first), MarketStatus::Open);
    assert_eq!(client.winning_outcome(&first), None);
    assert_eq!(client.pool(&first), 0);
    assert_eq!(client.redeemable_at(&first), 0);
}

#[test]
fn buying_yes_and_no_records_each_position_and_the_pool() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();

    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.buy(id, 1, Outcome::No, 50);

    assert_eq!(client.position(&id, &fixture.trader(0), &Outcome::Yes), 100);
    assert_eq!(client.position(&id, &fixture.trader(0), &Outcome::No), 0);
    assert_eq!(client.position(&id, &fixture.trader(1), &Outcome::No), 50);
    assert_eq!(client.pool(&id), 150);
    assert_eq!(fixture.escrow(), 150);
}

#[test]
fn repeated_purchases_accumulate_into_one_position() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();

    fixture.buy(id, 0, Outcome::Yes, 10);
    fixture.buy(id, 0, Outcome::Yes, 15);

    assert_eq!(client.position(&id, &fixture.trader(0), &Outcome::Yes), 25);
    assert_eq!(client.market(&id).yes_shares, 25);
    assert_eq!(fixture.escrow(), 25);
}

#[test]
fn buying_after_the_close_time_is_refused() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);

    assert!(client
        .try_buy(&id, &fixture.trader(0), &Outcome::Yes, &100)
        .is_err());
    assert_eq!(client.pool(&id), 0);
    assert_eq!(fixture.escrow(), 0);
}

#[test]
fn a_non_positive_purchase_is_refused() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();

    assert!(client
        .try_buy(&id, &fixture.trader(0), &Outcome::Yes, &0)
        .is_err());
    assert!(client
        .try_buy(&id, &fixture.trader(0), &Outcome::Yes, &-1)
        .is_err());
    assert_eq!(client.pool(&id), 0);
}

#[test]
fn a_price_at_the_threshold_resolves_yes() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.buy(id, 1, Outcome::No, 50);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);

    client.resolve(&id, &fixture.oracle, &100);

    assert_eq!(client.status(&id), MarketStatus::Resolved);
    assert_eq!(client.winning_outcome(&id), Some(Outcome::Yes));
    assert_eq!(client.redeemable_at(&id), CLOSE_TIME + DISPUTE_DELAY);
}

#[test]
fn a_price_below_the_threshold_resolves_no() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.buy(id, 1, Outcome::No, 50);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);

    client.resolve(&id, &fixture.oracle, &99);

    assert_eq!(client.winning_outcome(&id), Some(Outcome::No));
}

#[test]
fn resolving_before_the_close_time_is_refused() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.buy(id, 1, Outcome::No, 50);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME - 1);

    assert!(client.try_resolve(&id, &fixture.oracle, &100).is_err());
    assert_eq!(client.status(&id), MarketStatus::Open);
}

#[test]
fn only_the_market_oracle_can_resolve() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.buy(id, 1, Outcome::No, 50);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);

    // `mock_all_auths` makes the call authorized, so the contract's own check
    // that the caller is the configured oracle is what refuses it.
    assert!(client.try_resolve(&id, &fixture.trader(0), &100).is_err());
    assert_eq!(client.status(&id), MarketStatus::Open);
}

#[test]
fn a_market_cannot_be_resolved_twice() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.resolved(Outcome::Yes);

    assert!(client.try_resolve(&id, &fixture.oracle, &0).is_err());
    assert_eq!(client.winning_outcome(&id), Some(Outcome::Yes));
}

#[test]
fn resolving_a_one_sided_market_is_refused() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);

    // Winners can only be paid from the losing side, so a one-sided market has
    // nothing to distribute and resolution would strand the pool.
    assert!(client.try_resolve(&id, &fixture.oracle, &100).is_err());
    assert_eq!(client.status(&id), MarketStatus::Open);
}

#[test]
fn a_non_positive_oracle_price_is_refused() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 100);
    fixture.buy(id, 1, Outcome::No, 50);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);

    assert!(client.try_resolve(&id, &fixture.oracle, &0).is_err());
    assert_eq!(client.status(&id), MarketStatus::Open);
}

#[test]
fn claiming_before_the_dispute_window_closes_is_refused() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.resolved(Outcome::Yes);
    fixture
        .ctx
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + 1 + DISPUTE_DELAY - 1);

    assert!(client.try_claim(&id, &fixture.trader(0)).is_err());
    assert_eq!(fixture.escrow(), 150);
}

#[test]
fn a_winning_holder_claims_after_the_window_and_receives_the_pool() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.resolved(Outcome::Yes);
    fixture
        .ctx
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + 1 + DISPUTE_DELAY);

    let payout = client.claim(&id, &fixture.trader(0));

    // The only YES holder takes the whole pool: 100 YES collateral plus the 50
    // NO collateral that lost.
    assert_eq!(payout, 150);
    assert_eq!(
        fixture.token_client().balance(&fixture.trader(0)),
        10_000 - 100 + 150
    );
    assert_eq!(fixture.escrow(), 0);
    assert_eq!(client.market(&id).collateral_remaining, 0);
}

#[test]
fn a_losing_holder_has_nothing_to_claim() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.resolved(Outcome::Yes);
    fixture
        .ctx
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + 1 + DISPUTE_DELAY);

    assert!(client.try_claim(&id, &fixture.trader(1)).is_err());
    assert_eq!(fixture.escrow(), 150);
}

#[test]
fn a_winning_position_cannot_be_claimed_twice() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.resolved(Outcome::Yes);
    fixture
        .ctx
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + 1 + DISPUTE_DELAY);

    client.claim(&id, &fixture.trader(0));

    // The position was removed by the first claim, so the second has no shares.
    assert!(client.try_claim(&id, &fixture.trader(0)).is_err());
}

#[test]
fn two_winners_share_the_pool_with_the_remainder_going_to_the_last() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create();
    fixture.buy(id, 0, Outcome::Yes, 1);
    fixture.buy(id, 1, Outcome::Yes, 2);
    fixture.buy(id, 2, Outcome::No, 3);
    fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);
    client.resolve(&id, &fixture.oracle, &100);
    fixture
        .ctx
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + DISPUTE_DELAY);

    let first = client.claim(&id, &fixture.trader(0));
    let second = client.claim(&id, &fixture.trader(1));

    // 1/3 of 6 floors to 2; the final winner gets the rest, so the two payouts
    // add up to the whole pool rather than leaving rounding dust behind.
    assert_eq!(first, 2);
    assert_eq!(second, 4);
    assert_eq!(first + second, 6);
    assert_eq!(fixture.escrow(), 0);
}

#[test]
fn a_market_reports_every_field_of_its_record() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.resolved(Outcome::No);

    let market = client.market(&id);

    assert_eq!(market.token, fixture.token);
    assert_eq!(market.oracle, fixture.oracle);
    assert_eq!(market.threshold, 100);
    assert_eq!(market.close_time, CLOSE_TIME);
    assert_eq!(market.dispute_delay, DISPUTE_DELAY);
    assert_eq!(market.status, MarketStatus::Resolved);
    assert_eq!(market.outcome, Outcome::No);
    assert_eq!(market.pool, 150);
    assert_eq!(market.yes_shares, 100);
    assert_eq!(market.no_shares, 50);
    assert_eq!(market.winning_shares_remaining, 50);
    assert_eq!(market.collateral_remaining, 150);
}

#[test]
fn a_missing_market_is_an_error_at_the_contract_boundary() {
    let fixture = setup();
    let client = fixture.client();

    assert_eq!(client.next_market_id(), 1);
    assert!(client.try_pool(&99).is_err());
}
