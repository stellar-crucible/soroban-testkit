//! One crate per file: this suite drives the prediction market through
//! `soroban-testkit-generators`. Its central property is the one a market must
//! never break — the sum of every payout equals the collateral the market
//! collected, with nothing created and nothing stranded.
//!
//! Each case builds its own `Env`, so no case inherits another case's balances
//! or ledger clock.

extern crate std;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    Address, Env,
};
use soroban_testkit_example_prediction_market::{
    MarketStatus, Outcome, PredictionMarketContract, PredictionMarketContractClient,
};
use soroban_testkit_generators::strategies::{contract_address, token_amount};

const CLOSE_TIME: u64 = 1_000;
const DISPUTE_DELAY: u64 = 300;

struct Fixture {
    env: Env,
    contract: Address,
    token: Address,
    oracle: Address,
    yes_one: Address,
    yes_two: Address,
    no_one: Address,
}

/// Deploys a market and minted collateral for three traders.
fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();

    let contract = env.register(PredictionMarketContract, ());
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();

    let yes_one = Address::generate(&env);
    let yes_two = Address::generate(&env);
    let no_one = Address::generate(&env);

    Fixture {
        oracle: Address::generate(&env),
        yes_one,
        yes_two,
        no_one,
        contract,
        token,
        env,
    }
}

impl Fixture {
    fn client(&self) -> PredictionMarketContractClient<'_> {
        PredictionMarketContractClient::new(&self.env, &self.contract)
    }

    fn fund(&self, who: &Address, amount: i128) {
        StellarAssetClient::new(&self.env, &self.token).mint(who, &amount);
    }

    fn token_client(&self) -> soroban_sdk::token::Client<'_> {
        soroban_sdk::token::Client::new(&self.env, &self.token)
    }

    fn create(&self) -> u64 {
        self.client()
            .create(&self.token, &self.oracle, &100, &CLOSE_TIME, &DISPUTE_DELAY)
    }

    fn close(&self) {
        self.env.ledger().set_timestamp(CLOSE_TIME);
    }

    fn unlock(&self) {
        self.env.ledger().set_timestamp(CLOSE_TIME + DISPUTE_DELAY);
    }

    fn escrow(&self) -> i128 {
        self.token_client().balance(&self.contract)
    }

    fn balance(&self, who: &Address) -> i128 {
        self.token_client().balance(who)
    }
}

proptest! {
    // A case registers two contracts and runs several calls. The count stays
    // low enough that `cargo test --workspace` remains quick.
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn the_sum_of_payouts_equals_the_pool(
        yes_a in token_amount(),
        yes_b in token_amount(),
        no in token_amount(),
    ) {
        // The market is only meaningful with collateral on both sides.
        prop_assume!(yes_a > 0 && yes_b > 0 && no > 0);

        let fixture = setup();
        fixture.fund(&fixture.yes_one, yes_a);
        fixture.fund(&fixture.yes_two, yes_b);
        fixture.fund(&fixture.no_one, no);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &yes_a);
        client.buy(&id, &fixture.yes_two, &Outcome::Yes, &yes_b);
        client.buy(&id, &fixture.no_one, &Outcome::No, &no);
        fixture.close();
        client.resolve(&id, &fixture.oracle, &100);
        fixture.unlock();

        let first = client.claim(&id, &fixture.yes_one);
        let second = client.claim(&id, &fixture.yes_two);
        let pool = yes_a + yes_b + no;

        prop_assert_eq!(first + second, pool, "payouts must spend the whole pool");
        prop_assert_eq!(fixture.escrow(), 0, "no collateral may be stranded");
        prop_assert_eq!(client.market(&id).collateral_remaining, 0);
        prop_assert_eq!(client.market(&id).winning_shares_remaining, 0);
    }

    #[test]
    fn a_losing_position_is_never_paid(losing in token_amount()) {
        prop_assume!(losing > 0);

        let fixture = setup();
        fixture.fund(&fixture.yes_one, 100);
        fixture.fund(&fixture.no_one, losing);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &100);
        client.buy(&id, &fixture.no_one, &Outcome::No, &losing);
        fixture.close();
        client.resolve(&id, &fixture.oracle, &100);
        fixture.unlock();

        // The losing side keeps its shares and gets nothing.
        prop_assert!(client.try_claim(&id, &fixture.no_one).is_err());
        prop_assert_eq!(fixture.balance(&fixture.no_one), 0);
        prop_assert_eq!(fixture.escrow(), 100 + losing);
    }

    #[test]
    fn redemption_before_the_window_closes_is_always_refused(extra in 0u64..DISPUTE_DELAY) {
        let fixture = setup();
        fixture.fund(&fixture.yes_one, 100);
        fixture.fund(&fixture.no_one, 50);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &100);
        client.buy(&id, &fixture.no_one, &Outcome::No, &50);
        fixture.close();
        client.resolve(&id, &fixture.oracle, &100);
        fixture.env.ledger().set_timestamp(CLOSE_TIME + extra);

        prop_assert!(client.try_claim(&id, &fixture.yes_one).is_err());
        prop_assert_eq!(fixture.escrow(), 150);
    }

    #[test]
    fn the_boundary_price_decides_the_outcome(price in 1i128..=200i128) {
        let fixture = setup();
        fixture.fund(&fixture.yes_one, 100);
        fixture.fund(&fixture.no_one, 50);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &100);
        client.buy(&id, &fixture.no_one, &Outcome::No, &50);
        fixture.close();
        client.resolve(&id, &fixture.oracle, &price);

        let expected = if price >= 100 { Outcome::Yes } else { Outcome::No };
        prop_assert_eq!(client.winning_outcome(&id), Some(expected));
        prop_assert_eq!(client.status(&id), MarketStatus::Resolved);
    }

    #[test]
    fn purchases_after_the_close_time_are_refused(after in 0u64..=10_000u64) {
        let fixture = setup();
        fixture.fund(&fixture.yes_one, 100);

        let client = fixture.client();
        let id = fixture.create();
        fixture.env.ledger().set_timestamp(CLOSE_TIME + after);

        prop_assert!(client
            .try_buy(&id, &fixture.yes_one, &Outcome::Yes, &100)
            .is_err());
        prop_assert_eq!(client.pool(&id), 0);
        prop_assert_eq!(fixture.escrow(), 0);
    }

    #[test]
    fn a_resolution_from_anyone_but_the_oracle_is_refused(outsider in contract_address()) {
        let fixture = setup();
        fixture.fund(&fixture.yes_one, 100);
        fixture.fund(&fixture.no_one, 50);
        let outsider = outsider.to_address(&fixture.env);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &100);
        client.buy(&id, &fixture.no_one, &Outcome::No, &50);
        fixture.close();

        prop_assert!(client.try_resolve(&id, &outsider, &100).is_err());
        prop_assert_eq!(client.status(&id), MarketStatus::Open);
        prop_assert_eq!(fixture.escrow(), 150);
    }

    #[test]
    fn recorded_positions_always_add_up_to_the_pool(
        yes in token_amount(),
        no in token_amount(),
    ) {
        prop_assume!(yes > 0 && no > 0);

        let fixture = setup();
        fixture.fund(&fixture.yes_one, yes);
        fixture.fund(&fixture.no_one, no);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &yes);
        client.buy(&id, &fixture.no_one, &Outcome::No, &no);

        let market = client.market(&id);
        prop_assert_eq!(market.yes_shares + market.no_shares, market.pool);
        prop_assert_eq!(market.pool, yes + no);
        prop_assert_eq!(fixture.escrow(), market.pool);
        prop_assert_eq!(
            client.position(&id, &fixture.yes_one, &Outcome::Yes),
            yes
        );
        prop_assert_eq!(client.position(&id, &fixture.no_one, &Outcome::No), no);
    }

    #[test]
    fn resolution_needs_collateral_on_both_sides(amount in token_amount()) {
        prop_assume!(amount > 0);

        let fixture = setup();
        fixture.fund(&fixture.yes_one, amount);

        let client = fixture.client();
        let id = fixture.create();
        client.buy(&id, &fixture.yes_one, &Outcome::Yes, &amount);
        fixture.close();

        // With no losing collateral there is nothing to pay winners, so the
        // contract refuses to settle rather than stranding the pool.
        prop_assert!(client.try_resolve(&id, &fixture.oracle, &100).is_err());
        prop_assert_eq!(client.status(&id), MarketStatus::Open);
        prop_assert_eq!(fixture.escrow(), amount);
    }
}
