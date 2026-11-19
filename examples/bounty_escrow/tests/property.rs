//! One crate per file: this suite drives the bounty escrow through
//! `soroban-testkit-generators`, whose job is handing a test the inputs nobody
//! writes by hand — amounts, reviewer thresholds, deadlines and — for the
//! recovery block — every host error discriminant Soroban can report.
//!
//! Each case builds its own `Env`, so a case cannot inherit another case's
//! storage or token balances.

extern crate std;

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    xdr::ScErrorType,
    Address, Env, Error, Vec,
};
use soroban_testkit_example_bounty_escrow::{
    BountyEscrowContract, BountyEscrowContractClient, BountyStatus,
};
use soroban_testkit_generators::errors::host_error_code_coverage;
use soroban_testkit_generators::strategies::{contract_address, token_amount};

/// A deployed escrow, a funded token, and one identity per role.
struct Fixture {
    env: Env,
    contract: Address,
    token: Address,
    creator: Address,
    contributor: Address,
    reviewers: Vec<Address>,
}

/// Deploys the escrow and a Stellar asset contract holding `funding` for the
/// creator, with `reviewer_count` distinct reviewers.
fn setup(funding: i128, reviewer_count: usize) -> Fixture {
    let env = Env::default();
    env.mock_all_auths();

    let contract = env.register(BountyEscrowContract, ());
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let creator = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&creator, &funding);

    let mut reviewers = Vec::new(&env);
    for _ in 0..reviewer_count {
        reviewers.push_back(Address::generate(&env));
    }
    let contributor = Address::generate(&env);

    Fixture {
        env,
        contract,
        token,
        creator,
        contributor,
        reviewers,
    }
}

impl Fixture {
    fn client(&self) -> BountyEscrowContractClient<'_> {
        BountyEscrowContractClient::new(&self.env, &self.contract)
    }

    fn deadline(&self) -> u64 {
        self.env.ledger().timestamp() + 1_000
    }

    fn open(&self, amount: i128, threshold: u32) -> u64 {
        self.client().create(
            &self.creator,
            &self.token,
            &amount,
            &self.reviewers,
            &threshold,
            &self.deadline(),
        )
    }

    fn token_client(&self) -> soroban_sdk::token::Client<'_> {
        soroban_sdk::token::Client::new(&self.env, &self.token)
    }

    fn escrow(&self) -> i128 {
        self.token_client().balance(&self.contract)
    }

    fn paid_out(&self) -> i128 {
        self.token_client().balance(&self.contributor)
    }

    fn approve(&self, id: u64, index: u32) {
        self.client()
            .approve(&id, &self.reviewers.get(index).unwrap());
    }
}

/// A recovery decision, the shape a caller's `match` on a host failure produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Recovery {
    /// A resource or storage condition: the caller may try again.
    Retry,
    /// Not transient: stop and report it.
    Stop,
}

/// An error-recovery block over every host failure, total by construction.
fn recover(error: &Error) -> Recovery {
    if error.is_type(ScErrorType::Budget) || error.is_type(ScErrorType::Storage) {
        Recovery::Retry
    } else {
        Recovery::Stop
    }
}

proptest! {
    // A case registers two contracts and runs several calls, so the count stays
    // low enough that `cargo test --workspace` remains quick.
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn funds_are_released_only_at_the_reviewer_threshold(
        threshold in 1u32..=4u32,
        signatures in 0u32..=5u32,
    ) {
        let fixture = setup(1_000, 4);
        let client = fixture.client();
        let id = fixture.open(500, threshold);
        client.submit(&id, &fixture.contributor);

        // At most `threshold` signatures can be applied: once the bounty is
        // released the remaining reviewers are refused.
        let applied = signatures.min(threshold);
        for index in 0..applied {
            fixture.approve(id, index);
        }

        let bounty = client.get(&id);
        if signatures >= threshold {
            prop_assert_eq!(bounty.status, BountyStatus::Released);
            prop_assert_eq!(fixture.paid_out(), 500);
            prop_assert_eq!(fixture.escrow(), 0);
        } else {
            prop_assert_eq!(bounty.status, BountyStatus::Submitted);
            prop_assert_eq!(fixture.paid_out(), 0);
            prop_assert_eq!(fixture.escrow(), 500);
        }
        prop_assert_eq!(client.approval_count(&id), applied);
    }

    #[test]
    fn any_threshold_inside_the_reviewer_set_always_releases(
        reviewer_count in 1usize..=6,
        threshold in 1u32..=6u32,
    ) {
        prop_assume!(threshold as usize <= reviewer_count);

        let fixture = setup(1_000, reviewer_count);
        let client = fixture.client();
        let id = fixture.open(400, threshold);
        client.submit(&id, &fixture.contributor);

        for index in 0..threshold {
            fixture.approve(id, index);
        }

        prop_assert_eq!(client.get(&id).status, BountyStatus::Released);
        prop_assert_eq!(fixture.paid_out(), 400);
        prop_assert_eq!(client.approval_count(&id), threshold);
    }

    #[test]
    fn the_escrow_holds_exactly_the_bounty(
        amount in token_amount(),
        signatures in 0u32..=3u32,
    ) {
        // A zero-amount bounty is refused at creation, which is a different
        // property from the one under test here.
        prop_assume!(amount > 0);

        let fixture = setup(amount, 2);
        let client = fixture.client();
        let id = fixture.open(amount, 2);
        client.submit(&id, &fixture.contributor);
        for index in 0..signatures.min(2) {
            fixture.approve(id, index);
        }

        // Wherever the money is, the total has not changed: the escrow holds
        // what the contributor has not been paid yet.
        prop_assert_eq!(fixture.escrow() + fixture.paid_out(), amount);
        prop_assert_eq!(fixture.token_client().balance(&fixture.creator), 0);
    }

    #[test]
    fn a_refund_after_the_deadline_returns_the_escrow(
        amount in token_amount(),
        extra in 0u64..=10_000u64,
    ) {
        prop_assume!(amount > 0);

        let fixture = setup(amount, 2);
        let deadline = fixture.deadline();
        let id = fixture.open(amount, 2);
        fixture.env.ledger().set_timestamp(deadline + extra);

        fixture.client().refund(&id);

        prop_assert_eq!(fixture.client().get(&id).status, BountyStatus::Refunded);
        prop_assert_eq!(fixture.token_client().balance(&fixture.creator), amount);
        prop_assert_eq!(fixture.escrow(), 0);
    }

    #[test]
    fn a_submission_after_the_deadline_is_refused(extra in 0u64..=10_000u64) {
        let fixture = setup(1_000, 2);
        let deadline = fixture.deadline();
        let id = fixture.open(500, 2);
        fixture.env.ledger().set_timestamp(deadline + extra);

        // At the deadline exactly, `now < deadline` is already false, so the
        // contract closes submissions when the clock reaches the timestamp.
        prop_assert!(fixture.client().try_submit(&id, &fixture.contributor).is_err());
        prop_assert_eq!(fixture.client().get(&id).status, BountyStatus::Open);
        prop_assert_eq!(fixture.escrow(), 500);
    }

    #[test]
    fn an_approval_after_the_deadline_is_refused(extra in 0u64..=10_000u64) {
        let fixture = setup(1_000, 3);
        let deadline = fixture.deadline();
        let id = fixture.open(500, 3);
        fixture.client().submit(&id, &fixture.contributor);
        fixture.approve(id, 0);
        fixture.env.ledger().set_timestamp(deadline + extra);

        // One signature short of the release, and the clock has closed the
        // window: the escrow stays put for the refund instead.
        prop_assert!(fixture
            .client()
            .try_approve(&id, &fixture.reviewers.get(1).unwrap())
            .is_err());
        prop_assert_eq!(fixture.client().approval_count(&id), 1);
        prop_assert_eq!(fixture.escrow(), 500);
    }

    #[test]
    fn a_second_submission_is_refused(second in contract_address()) {
        let fixture = setup(1_000, 2);
        let id = fixture.open(500, 2);
        fixture.client().submit(&id, &fixture.contributor);

        let second = second.to_address(&fixture.env);
        prop_assert!(fixture.client().try_submit(&id, &second).is_err());
        prop_assert_eq!(
            fixture.client().get(&id).contributor,
            Some(fixture.contributor.clone())
        );
    }

    #[test]
    fn a_settled_bounty_refuses_further_moves(extra in 0u64..=1_000u64) {
        let fixture = setup(1_000, 1);
        let deadline = fixture.deadline();
        let id = fixture.open(500, 1);
        fixture.client().submit(&id, &fixture.contributor);
        fixture.approve(id, 0);

        fixture.env.ledger().set_timestamp(deadline + extra);

        // Once released, neither a late signature nor a refund can move the
        // same funds a second time.
        prop_assert!(fixture
            .client()
            .try_approve(&id, &fixture.reviewers.get(0).unwrap())
            .is_err());
        prop_assert!(fixture.client().try_refund(&id).is_err());
        prop_assert_eq!(fixture.paid_out(), 500);
        prop_assert_eq!(fixture.escrow(), 0);
    }

    #[test]
    fn every_generated_host_error_is_recoverable_around_a_rejected_action(
        error in host_error_code_coverage(),
    ) {
        let fixture = setup(1_000, 2);
        let client = fixture.client();
        let id = fixture.open(500, 2);
        client.submit(&id, &fixture.contributor);
        let outsider = Address::generate(&fixture.env);

        // The rejection is what leaves the contract's state intact.
        prop_assert!(client.try_approve(&id, &outsider).is_err());

        // The recovery block is fuzzed over the whole host error domain: every
        // category gets a decision instead of a panic, and only the transient
        // categories ask for a retry.
        let decision = recover(&error.as_error());
        match error.error_type() {
            ScErrorType::Budget | ScErrorType::Storage => {
                prop_assert_eq!(decision, Recovery::Retry)
            }
            _ => prop_assert_eq!(decision, Recovery::Stop),
        }
        prop_assert!(matches!(
            error.error_type(),
            ScErrorType::Contract
                | ScErrorType::WasmVm
                | ScErrorType::Context
                | ScErrorType::Storage
                | ScErrorType::Object
                | ScErrorType::Crypto
                | ScErrorType::Events
                | ScErrorType::Budget
                | ScErrorType::Value
                | ScErrorType::Auth
        ));

        prop_assert_eq!(client.approval_count(&id), 0);
        prop_assert_eq!(fixture.escrow(), 500);
    }
}
