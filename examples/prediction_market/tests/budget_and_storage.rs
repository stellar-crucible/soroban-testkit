//! One crate per file: this suite drives the prediction market through
//! `soroban-testkit-core`, which names what a call cost and which tier a key
//! was written to.
//!
//! The market keeps its whole record under one persistent key, so most of these
//! tests are about a state transition rewriting that key and nothing else.

extern crate std;

use std::vec::Vec as StdVec;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    Address, Env,
};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_prediction_market::{
    MarketStatus, Outcome, PredictionMarketContract, PredictionMarketContractClient,
};

const CLOSE_TIME: u64 = 1_000;
const DISPUTE_DELAY: u64 = 300;

struct MarketFixture {
    env: Env,
    contract: Address,
    token: Address,
    oracle: Address,
    yes_buyer: Address,
    no_buyer: Address,
}

fn setup() -> MarketFixture {
    let env = Env::default();
    env.mock_all_auths();

    let contract = env.register(PredictionMarketContract, ());
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let yes_buyer = Address::generate(&env);
    let no_buyer = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&yes_buyer, &1_000_000);
    StellarAssetClient::new(&env, &token).mint(&no_buyer, &1_000_000);

    MarketFixture {
        oracle: Address::generate(&env),
        yes_buyer,
        no_buyer,
        contract,
        token,
        env,
    }
}

impl MarketFixture {
    fn client(&self) -> PredictionMarketContractClient<'_> {
        PredictionMarketContractClient::new(&self.env, &self.contract)
    }

    fn create(&self, close_time: u64) -> u64 {
        self.client()
            .create(&self.token, &self.oracle, &100, &close_time, &DISPUTE_DELAY)
    }

    fn two_sided(&self) -> u64 {
        let id = self.create(CLOSE_TIME);
        self.client().buy(&id, &self.yes_buyer, &Outcome::Yes, &100);
        self.client().buy(&id, &self.no_buyer, &Outcome::No, &50);
        id
    }

    fn resolved(&self, outcome: Outcome) -> u64 {
        let id = self.two_sided();
        self.env.ledger().set_timestamp(CLOSE_TIME);
        let price = if outcome == Outcome::Yes { 100 } else { 99 };
        self.client().resolve(&id, &self.oracle, &price);
        id
    }

    fn storage(&self) -> StorageSnapshot {
        StorageSnapshot::capture(&self.env, &self.contract)
    }

    fn token_client(&self) -> soroban_sdk::token::Client<'_> {
        soroban_sdk::token::Client::new(&self.env, &self.token)
    }
}

#[test]
fn creating_a_market_stays_inside_its_transaction_ceiling() {
    let fixture = setup();

    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "create",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || fixture.create(CLOSE_TIME)
    );
}

#[test]
fn buying_shares_stays_inside_its_ceiling() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create(CLOSE_TIME);

    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "buy",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || client.buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100)
    );

    assert_eq!(client.pool(&id), 100);
}

#[test]
fn redeeming_a_winning_position_stays_inside_its_ceiling() {
    let fixture = setup();
    let id = fixture.resolved(Outcome::Yes);
    fixture
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + DISPUTE_DELAY);

    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "claim",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || fixture.client().claim(&id, &fixture.yes_buyer)
    );

    assert_eq!(
        fixture.token_client().balance(&fixture.yes_buyer),
        1_000_050
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=create metric=cpu_insns")]
fn a_call_past_its_ceiling_reports_a_parseable_line() {
    let fixture = setup();

    soroban_testkit_core::budget_guard!(&fixture.env, "create", { cpu_max: 1 }, || {
        fixture.create(CLOSE_TIME)
    });
}

#[test]
fn reading_a_market_costs_less_than_buying_into_it() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create(CLOSE_TIME);

    client.market(&id);
    let read = BudgetSnapshot::last_invocation(&fixture.env);

    client.buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100);
    let buy = BudgetSnapshot::last_invocation(&fixture.env);

    assert!(
        read.cpu_insns < buy.cpu_insns,
        "a purchase ({}) costing no more than a read ({}) cannot be moving collateral",
        buy.cpu_insns,
        read.cpu_insns
    );
    assert!(read.diff(&buy).cpu_insns > 0);
}

#[test]
fn resolving_costs_more_than_reading_the_market() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.two_sided();
    fixture.env.ledger().set_timestamp(CLOSE_TIME);

    client.market(&id);
    let read = BudgetSnapshot::last_invocation(&fixture.env);
    client.resolve(&id, &fixture.oracle, &100);
    let resolve = BudgetSnapshot::last_invocation(&fixture.env);

    assert!(resolve.cpu_insns > read.cpu_insns);
    assert_eq!(client.status(&id), MarketStatus::Resolved);
}

#[test]
fn create_writes_the_counter_to_instance_and_the_market_to_persistent() {
    let fixture = setup();

    let id = fixture.create(CLOSE_TIME);
    assert_eq!(id, 1);

    soroban_testkit_core::assert_storage_tier!(
        &fixture.env,
        &fixture.contract,
        "next_id",
        StorageTier::Instance
    );
    soroban_testkit_core::assert_storage_tier!(
        &fixture.env,
        &fixture.contract,
        "markets",
        StorageTier::Persistent
    );
    assert_eq!(fixture.storage().len(), 2);
}

#[test]
fn a_purchase_rewrites_only_the_market_record() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create(CLOSE_TIME);

    let before = fixture.storage();
    client.buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100);
    let after = fixture.storage();

    let diff = before.diff(&after);
    assert!(diff.added().is_empty(), "no new key: {:?}", diff.added());
    assert!(
        diff.removed().is_empty(),
        "no removed key: {:?}",
        diff.removed()
    );
    assert_eq!(
        diff.modified()
            .iter()
            .map(|change| change.key.clone())
            .collect::<StdVec<_>>(),
        ["markets"]
    );
    assert_eq!(after.len(), 2);
}

#[test]
fn resolution_transitions_the_status_in_the_stored_value() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.two_sided();
    fixture.env.ledger().set_timestamp(CLOSE_TIME);

    let before = fixture.storage();
    client.resolve(&id, &fixture.oracle, &100);
    let after = fixture.storage();

    let diff = before.diff(&after);
    assert_eq!(diff.len(), 1);
    assert_eq!(diff.modified()[0].key, "markets");
    assert!(
        diff.modified()[0].before.contains("Open"),
        "the old record should still read Open: {}",
        diff.modified()[0].before
    );
    assert!(
        diff.modified()[0].after.contains("Resolved"),
        "the new record should read Resolved: {}",
        diff.modified()[0].after
    );
    assert_eq!(client.status(&id), MarketStatus::Resolved);
}

#[test]
fn redemption_reduces_the_remaining_collateral_without_touching_the_pool() {
    let fixture = setup();
    let id = fixture.resolved(Outcome::Yes);
    fixture
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + DISPUTE_DELAY);

    let before = fixture.storage();
    let payout = fixture.client().claim(&id, &fixture.yes_buyer);
    let after = fixture.storage();

    assert_eq!(payout, 150);
    let market = fixture.client().market(&id);
    assert_eq!(market.pool, 150, "the recorded pool is the total deposited");
    assert_eq!(market.collateral_remaining, 0);
    assert_eq!(market.winning_shares_remaining, 0);
    let diff = before.diff(&after);
    assert_eq!(diff.len(), 1);
    assert_eq!(diff.modified()[0].key, "markets");
}

#[test]
fn a_refused_purchase_leaves_storage_untouched() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.create(CLOSE_TIME);
    fixture.env.ledger().set_timestamp(CLOSE_TIME);

    let before = fixture.storage();
    assert!(client
        .try_buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100)
        .is_err());

    before.assert_unchanged(&fixture.storage());
}

#[test]
fn reads_leave_storage_untouched() {
    let fixture = setup();
    let client = fixture.client();
    let id = fixture.two_sided();

    let before = fixture.storage();
    client.market(&id);
    client.status(&id);
    client.pool(&id);
    client.position(&id, &fixture.yes_buyer, &Outcome::Yes);
    client.next_market_id();

    before.assert_unchanged(&fixture.storage());
}

#[test]
fn a_baseline_records_one_case_per_lifecycle_step() {
    let fixture = setup();
    let client = fixture.client();
    let mut baseline = BudgetBaseline::new();

    let id = fixture.create(CLOSE_TIME);
    baseline.record(
        "market/create",
        BudgetSnapshot::last_invocation(&fixture.env),
    );
    client.buy(&id, &fixture.yes_buyer, &Outcome::Yes, &100);
    baseline.record("market/buy", BudgetSnapshot::last_invocation(&fixture.env));
    client.buy(&id, &fixture.no_buyer, &Outcome::No, &50);
    fixture.env.ledger().set_timestamp(CLOSE_TIME);
    client.resolve(&id, &fixture.oracle, &100);
    baseline.record(
        "market/resolve",
        BudgetSnapshot::last_invocation(&fixture.env),
    );
    fixture
        .env
        .ledger()
        .set_timestamp(CLOSE_TIME + DISPUTE_DELAY);
    client.claim(&id, &fixture.yes_buyer);
    baseline.record(
        "market/claim",
        BudgetSnapshot::last_invocation(&fixture.env),
    );

    let reloaded = BudgetBaseline::parse(&baseline.to_json_string())
        .expect("a baseline this crate wrote parses");
    assert_eq!(reloaded.len(), 4);
    for (case, cost) in reloaded.cases() {
        reloaded
            .guard(case)
            .tolerance_percent(0)
            .assert_within(&cost);
    }
}
