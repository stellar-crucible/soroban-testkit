//! One crate per file: this suite drives the prediction market through
//! `soroban-testkit-assert`, which reads the events a market publishes and the
//! authorization each state change requires.
//!
//! Every assertion stays on the lifecycle's observable surface — topics and
//! payload fields — rather than on the contract's internal storage.

extern crate std;

use std::vec::Vec as StdVec;

use soroban_sdk::{testutils::Ledger as _, token::StellarAssetClient, Address};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_prediction_market::{
    Outcome, PredictionMarketContract, PredictionMarketContractClient,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const CLOSE_TIME: u64 = 1_000;
const DISPUTE_DELAY: u64 = 300;

struct MarketFixture {
    ctx: TestContext,
    contract: Address,
    token: Address,
    oracle: Address,
    yes_buyer: Address,
    no_buyer: Address,
}

fn setup() -> MarketFixture {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let contract = ctx.env.register(PredictionMarketContract, ());
    let token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let yes_buyer = ctx.user(0).clone();
    let no_buyer = ctx.user(1).clone();
    StellarAssetClient::new(&ctx.env, &token).mint(&yes_buyer, &10_000);
    StellarAssetClient::new(&ctx.env, &token).mint(&no_buyer, &10_000);

    MarketFixture {
        oracle: ctx.user(4).clone(),
        yes_buyer,
        no_buyer,
        contract,
        token,
        ctx,
    }
}

impl MarketFixture {
    fn client(&self) -> PredictionMarketContractClient<'_> {
        PredictionMarketContractClient::new(&self.ctx.env, &self.contract)
    }

    fn create(&self) -> u64 {
        self.client()
            .create(&self.token, &self.oracle, &100, &CLOSE_TIME, &DISPUTE_DELAY)
    }

    fn two_sided(&self) -> u64 {
        let id = self.create();
        self.client().buy(&id, &self.yes_buyer, &Outcome::Yes, &100);
        self.client().buy(&id, &self.no_buyer, &Outcome::No, &50);
        id
    }

    fn resolved(&self, outcome: Outcome) -> u64 {
        let id = self.two_sided();
        self.close();
        let price = if outcome == Outcome::Yes { 100 } else { 99 };
        self.client().resolve(&id, &self.oracle, &price);
        id
    }

    fn close(&self) {
        self.ctx.env.ledger().set_timestamp(CLOSE_TIME);
    }

    fn unlock(&self) {
        self.ctx
            .env
            .ledger()
            .set_timestamp(CLOSE_TIME + DISPUTE_DELAY);
    }
}

#[test]
fn a_created_event_carries_the_market_terms() {
    let fixture = setup();
    let id = fixture.create();

    EventMatcher::new(&fixture.ctx.env)
        .from_contract(&fixture.contract)
        .with_topic("created")
        .assert_data_matches(|data| {
            data.field("id")
                .and_then(|value| value.deserialize::<u64>())
                == Some(id)
                && data
                    .field("oracle")
                    .and_then(|value| value.deserialize::<Address>())
                    == Some(fixture.oracle.clone())
                && data
                    .field("threshold")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(100)
                && data
                    .field("close_time")
                    .and_then(|value| value.deserialize::<u64>())
                    == Some(CLOSE_TIME)
                && data
                    .field("dispute_delay")
                    .and_then(|value| value.deserialize::<u64>())
                    == Some(DISPUTE_DELAY)
        });
}

#[test]
fn a_yes_purchase_event_names_the_side_and_the_amount() {
    let fixture = setup();
    let id = fixture.create();
    fixture
        .client()
        .buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100);

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("bought")
        .assert_data_matches(|data| {
            data.field("id")
                .and_then(|value| value.deserialize::<u64>())
                == Some(id)
                && data
                    .field("buyer")
                    .and_then(|value| value.deserialize::<Address>())
                    == Some(fixture.yes_buyer.clone())
                && data
                    .field("outcome")
                    .and_then(|value| value.deserialize::<Outcome>())
                    == Some(Outcome::Yes)
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(100)
        });
}

#[test]
fn a_no_purchase_event_carries_the_other_outcome() {
    let fixture = setup();
    let id = fixture.create();
    fixture
        .client()
        .buy(&id, &fixture.no_buyer, &Outcome::No, &50);

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("bought")
        .assert_data_matches(|data| {
            data.field("outcome")
                .and_then(|value| value.deserialize::<Outcome>())
                == Some(Outcome::No)
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(50)
        });
}

#[test]
fn a_resolution_event_carries_the_outcome_and_the_unlock_time() {
    let fixture = setup();
    let id = fixture.resolved(Outcome::Yes);

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("resolved")
        .assert_data_matches(|data| {
            data.field("id")
                .and_then(|value| value.deserialize::<u64>())
                == Some(id)
                && data
                    .field("outcome")
                    .and_then(|value| value.deserialize::<Outcome>())
                    == Some(Outcome::Yes)
                && data
                    .field("pool")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(150)
                && data
                    .field("redeemable_at")
                    .and_then(|value| value.deserialize::<u64>())
                    == Some(CLOSE_TIME + DISPUTE_DELAY)
        });
}

#[test]
fn a_redemption_event_reports_the_shares_and_the_payout() {
    let fixture = setup();
    let id = fixture.resolved(Outcome::Yes);
    fixture.unlock();

    fixture.client().claim(&id, &fixture.yes_buyer);

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("redeemed")
        .assert_data_matches(|data| {
            data.field("holder")
                .and_then(|value| value.deserialize::<Address>())
                == Some(fixture.yes_buyer.clone())
                && data
                    .field("shares")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(100)
                && data
                    .field("payout")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(150)
        });
}

#[test]
fn a_refused_redemption_publishes_no_redemption_event() {
    let fixture = setup();
    let id = fixture.resolved(Outcome::Yes);

    // The dispute window is still open, so the claim is rejected before it can
    // publish anything.
    assert!(fixture.client().try_claim(&id, &fixture.yes_buyer).is_err());

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("redeemed")
        .assert_not_emitted();
}

#[test]
fn the_lifecycle_events_arrive_in_order() {
    let fixture = setup();
    let mut log = EventLog::new(&fixture.ctx.env);

    let id = fixture.create();
    log.collect();
    fixture
        .client()
        .buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100);
    log.collect();
    fixture
        .client()
        .buy(&id, &fixture.no_buyer, &Outcome::No, &50);
    log.collect();
    fixture.close();
    fixture.client().resolve(&id, &fixture.oracle, &100);
    log.collect();
    fixture.unlock();
    fixture.client().claim(&id, &fixture.yes_buyer);
    log.collect();

    // The token contract publishes its own transfers alongside the market
    // events, so filter on the market's leading topic before reading the
    // sequence back.
    let market_events: StdVec<String> = log
        .topics()
        .iter()
        .filter(|topics| topics.first().map(String::as_str) == Some("market"))
        .map(|topics| topics[1].clone())
        .collect();
    assert_eq!(
        market_events,
        ["created", "bought", "bought", "resolved", "redeemed"]
    );
    log.matcher().with_topic("resolved").assert_count(1);
    log.matcher().with_topic("redeemed").assert_count(1);
}

#[test]
fn buying_records_the_buyers_authorization() {
    let fixture = setup();
    let id = fixture.create();

    fixture
        .client()
        .buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100);

    assert_auth_matches!(fixture.ctx.env, {
        invoker: fixture.yes_buyer,
        function: "buy",
        args: [id, _, _, 100i128],
    });
}

#[test]
fn resolving_records_the_oracles_authorization() {
    let fixture = setup();
    let id = fixture.two_sided();
    fixture.close();

    fixture.client().resolve(&id, &fixture.oracle, &100);

    assert_auth_matches!(fixture.ctx.env, {
        invoker: fixture.oracle,
        function: "resolve",
        args: [id, _, 100i128],
    });
}

#[test]
fn claiming_records_the_holders_authorization() {
    let fixture = setup();
    let id = fixture.resolved(Outcome::Yes);
    fixture.unlock();

    fixture.client().claim(&id, &fixture.yes_buyer);

    assert_auth_matches!(fixture.ctx.env, {
        invoker: fixture.yes_buyer,
        function: "claim",
        args: [id, _],
    });
}

#[test]
fn a_read_requires_no_authorization() {
    let fixture = setup();
    let id = fixture.two_sided();

    fixture.client().market(&id);

    soroban_testkit_assert::auth::AuthMatcher::new(&fixture.ctx.env).assert_no_auth_required();
}
