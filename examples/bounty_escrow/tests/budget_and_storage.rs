//! One crate per file: this suite drives the bounty escrow through
//! `soroban-testkit-core`, whose two jobs here are naming what a call cost and
//! which storage tier a key was written to.
//!
//! The harness is a bare `Env` rather than a `TestContext`, so nothing in these
//! tests reaches for another testkit crate — `src/test.rs` is where the fixture
//! builder is demonstrated.

extern crate std;

use std::vec::Vec as StdVec;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    token::StellarAssetClient,
    Address, Env, Vec,
};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_bounty_escrow::{
    BountyEscrowContract, BountyEscrowContractClient, BountyStatus,
};

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
fn setup(reviewer_count: usize) -> Fixture {
    let env = Env::default();
    env.mock_all_auths();

    let contract = env.register(BountyEscrowContract, ());
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let creator = Address::generate(&env);
    StellarAssetClient::new(&env, &token).mint(&creator, &1_000_000);

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

    /// Opens a bounty whose threshold is the whole reviewer set.
    fn open(&self, amount: i128) -> u64 {
        self.client().create(
            &self.creator,
            &self.token,
            &amount,
            &self.reviewers,
            &self.reviewers.len(),
            &self.deadline(),
        )
    }

    fn token_client(&self) -> soroban_sdk::token::Client<'_> {
        soroban_sdk::token::Client::new(&self.env, &self.token)
    }

    fn storage(&self) -> StorageSnapshot {
        StorageSnapshot::capture(&self.env, &self.contract)
    }
}

#[test]
fn funding_a_bounty_stays_inside_its_transaction_ceiling() {
    let fixture = setup(3);

    // A ceiling is transaction headroom, not the measured cost. A contract
    // registered natively is metered without the VM's own costs, so the create
    // below reads tens of thousands of instructions and a network would bill
    // more.
    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "create",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || fixture.open(1_000)
    );
}

#[test]
fn a_multi_signature_release_stays_inside_its_ceiling() {
    let fixture = setup(2);
    let client = fixture.client();
    let id = fixture.open(1_000);
    client.submit(&id, &fixture.contributor);
    client.approve(&id, &fixture.reviewers.get(0).unwrap());

    // The last approval is the expensive one: it records the second signature
    // and moves the escrow, so it is measured on its own.
    soroban_testkit_core::budget_guard!(
        &fixture.env,
        "release",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || client.approve(&id, &fixture.reviewers.get(1).unwrap())
    );

    assert_eq!(fixture.token_client().balance(&fixture.contributor), 1_000);
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=create metric=cpu_insns")]
fn a_call_past_its_ceiling_fails_the_test_with_a_parseable_line() {
    let fixture = setup(1);

    soroban_testkit_core::budget_guard!(&fixture.env, "create", { cpu_max: 1 }, || {
        fixture.open(1_000)
    });
}

#[test]
fn reading_the_bounty_costs_less_than_releasing_it() {
    let fixture = setup(1);
    let client = fixture.client();
    let id = fixture.open(1_000);
    client.submit(&id, &fixture.contributor);

    client.get(&id);
    let read = BudgetSnapshot::last_invocation(&fixture.env);

    client.approve(&id, &fixture.reviewers.get(0).unwrap());
    let release = BudgetSnapshot::last_invocation(&fixture.env);

    // The reading belongs to the call that just happened, so the two numbers
    // describe a read and a payout rather than one cumulative total.
    assert!(
        read.cpu_insns < release.cpu_insns,
        "a release ({}) costing no more than a read ({}) cannot be paying out",
        release.cpu_insns,
        read.cpu_insns
    );
    assert!(read.diff(&release).cpu_insns > 0);
}

/// The total cost of collecting `reviewer_count` signatures, release included.
///
/// Summing `last_invocation` across the approvals is what makes this a payout
/// benchmark rather than the cost of one call: the release happens on the last
/// approval, and every signature before it is part of what a bounty costs.
fn payout_cost(reviewer_count: usize) -> BudgetSnapshot {
    let fixture = setup(reviewer_count);
    let client = fixture.client();
    let id = fixture.open(1_000);
    client.submit(&id, &fixture.contributor);

    let mut total = BudgetSnapshot {
        cpu_insns: 0,
        mem_bytes: 0,
    };
    for reviewer in fixture.reviewers.iter() {
        client.approve(&id, &reviewer);
        let step = BudgetSnapshot::last_invocation(&fixture.env);
        total.cpu_insns += step.cpu_insns;
        total.mem_bytes += step.mem_bytes;
    }
    total
}

#[test]
fn payout_cost_grows_with_the_reviewer_count() {
    let measured: StdVec<(usize, BudgetSnapshot)> = [1usize, 2, 3, 5, 7]
        .iter()
        .map(|&n| (n, payout_cost(n)))
        .collect();

    // A benchmark is a recorded measurement, not a threshold: the table is what
    // a reviewer compares a change against.
    for (count, cost) in &measured {
        println!(
            "reviewers={count:>2} cpu_insns={} mem_bytes={}",
            cost.cpu_insns, cost.mem_bytes
        );
    }

    for pair in measured.windows(2) {
        assert!(
            pair[1].1.cpu_insns > pair[0].1.cpu_insns,
            "{} reviewers costing {} instructions cannot be cheaper than {} costing {}",
            pair[1].0,
            pair[1].1.cpu_insns,
            pair[0].0,
            pair[0].1.cpu_insns
        );
    }
    assert!(
        measured.last().unwrap().1.mem_bytes > measured.first().unwrap().1.mem_bytes,
        "collecting seven signatures must write more than collecting one"
    );
}

#[test]
fn a_baseline_records_one_payout_case_per_reviewer_count() {
    let mut baseline = BudgetBaseline::new();
    for count in [1usize, 3, 5] {
        baseline.record(format!("payout/{count}"), payout_cost(count));
    }

    // Round-tripping through the serialised form is what lets a pull request
    // compare its costs against a file recorded on `main`.
    let reloaded = BudgetBaseline::parse(&baseline.to_json_string())
        .expect("a baseline this crate wrote parses");
    assert_eq!(reloaded.len(), 3);
    assert_eq!(reloaded.get("payout/3"), baseline.get("payout/3"));

    let recorded = baseline.get("payout/5").unwrap();
    baseline
        .guard("payout/5")
        .tolerance_percent(0)
        .assert_within(&recorded);
}

#[test]
fn create_writes_the_counter_to_instance_and_the_map_to_persistent() {
    let fixture = setup(2);

    let id = fixture.open(500);
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
        "bounties",
        StorageTier::Persistent
    );
    assert_eq!(fixture.storage().len(), 2);
}

#[test]
fn the_first_create_adds_both_of_the_keys_it_owns() {
    let fixture = setup(2);

    let before = fixture.storage();
    fixture.open(500);
    let after = fixture.storage();

    before.assert_entry_added(&after, "next_id");
    before.assert_entry_added(&after, "bounties");
    assert_eq!(
        after
            .entries()
            .iter()
            .map(|entry| entry.qualified())
            .collect::<StdVec<_>>(),
        ["instance:next_id", "persistent:bounties"]
    );
}

#[test]
fn later_transitions_rewrite_the_map_without_adding_a_key() {
    let fixture = setup(1);
    let client = fixture.client();
    let id = fixture.open(500);
    client.submit(&id, &fixture.contributor);

    let before = fixture.storage();
    client.approve(&id, &fixture.reviewers.get(0).unwrap());
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
        ["bounties"],
        "the release rewrites the bounty record and leaves the counter alone"
    );
    assert_eq!(after.len(), 2);
}

#[test]
fn reading_a_bounty_leaves_storage_unchanged() {
    let fixture = setup(2);
    let client = fixture.client();
    let id = fixture.open(500);

    let before = fixture.storage();
    client.get(&id);
    client.status(&id);
    client.approvals(&id);
    client.reviewers(&id);
    client.next_bounty_id();

    before.assert_unchanged(&fixture.storage());
    assert_eq!(client.status(&id), BountyStatus::Open);
}

#[test]
fn a_refund_rewrites_only_the_bounty_record() {
    let fixture = setup(2);
    let client = fixture.client();
    let id = fixture.open(500);
    client.submit(&id, &fixture.contributor);
    fixture.env.ledger().set_timestamp(fixture.deadline());

    let before = fixture.storage();
    client.refund(&id);
    let after = fixture.storage();

    let diff = before.diff(&after);
    assert!(!diff.is_empty(), "a refund writes the new status");
    assert_eq!(diff.len(), 1);
    assert_eq!(diff.modified()[0].key, "bounties");
    assert_eq!(after.in_tier(StorageTier::Instance).len(), 1);
}

#[test]
#[should_panic(
    expected = "Storage tier mismatch for key 'next_id': expected persistent, found instance"
)]
fn a_key_in_the_wrong_tier_is_a_failure() {
    let fixture = setup(1);
    fixture.open(500);

    // The counter is instance storage: a test that expects it to survive like a
    // persistent entry is wrong about the contract and says so.
    soroban_testkit_core::assert_storage_tier!(
        &fixture.env,
        &fixture.contract,
        "next_id",
        StorageTier::Persistent
    );
}
