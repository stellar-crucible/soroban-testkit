//! One crate per file: this suite drives the registry through
//! `soroban-testkit-core`, whose two jobs here are naming what a call cost and
//! which storage tier a key was written to.
//!
//! The harness is a bare `Env` rather than a `TestContext`, so nothing in these
//! tests reaches for another testkit crate — `src/test.rs` is where the fixture
//! builder is demonstrated.

extern crate std;

use std::{string::String as StdString, vec::Vec};

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, String};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::storage::{inspect_storage, StorageSnapshot, StorageTier};
use soroban_testkit_example_domain_service::{DomainService, DomainServiceClient, RecordKind};

const GRACE: u64 = 604_800;

fn deployed() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(DomainService, ());
    let admin = Address::generate(&env);
    DomainServiceClient::new(&env, &contract).initialize(&admin, &admin, &100, &GRACE);
    (env, contract)
}

fn client<'a>(env: &'a Env, contract: &Address) -> DomainServiceClient<'a> {
    DomainServiceClient::new(env, contract)
}

fn name(env: &Env, text: &str) -> String {
    String::from_str(env, text)
}

#[test]
fn a_registration_stays_inside_its_transaction_ceiling() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    client(&env, &contract).deposit(&owner, &1_000);

    // A ceiling is transaction headroom, not this call's measured cost: a
    // contract registered natively is metered without the VM's own costs, so a
    // registration reads tens of thousands of instructions and a network would
    // bill more.
    soroban_testkit_core::budget_guard!(
        &env,
        "register",
        { cpu_max: 20_000_000, mem_max: 20_000_000 },
        || client(&env, &contract).register(&owner, &name(&env, "alice"), &1)
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=register metric=cpu_insns")]
fn a_registration_past_its_ceiling_fails_the_test_with_a_parseable_line() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    client(&env, &contract).deposit(&owner, &1_000);

    soroban_testkit_core::budget_guard!(&env, "register", { cpu_max: 1 }, || {
        client(&env, &contract).register(&owner, &name(&env, "alice"), &1)
    });
}

#[test]
fn initialize_writes_only_instance_storage() {
    let (env, contract) = deployed();

    // The operator, the treasury and the two pricing terms are configuration:
    // they live for as long as the contract instance does.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "treasury", StorageTier::Instance);
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "base_fee", StorageTier::Instance);
    soroban_testkit_core::assert_storage_tier!(
        &env,
        &contract,
        "grace_period",
        StorageTier::Instance
    );

    let snapshot = StorageSnapshot::capture(&env, &contract);
    assert_eq!(snapshot.len(), 4);
    assert!(snapshot.in_tier(StorageTier::Persistent).is_empty());
}

#[test]
fn the_first_registration_opens_persistent_storage() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    client(&env, &contract).deposit(&owner, &1_000);

    let before = StorageSnapshot::capture(&env, &contract);
    client(&env, &contract).register(&owner, &name(&env, "alice"), &1);
    let after = StorageSnapshot::capture(&env, &contract);

    before.assert_entry_added(&after, "domains");
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "domains", StorageTier::Persistent);
}

#[test]
fn every_registration_grows_the_one_registry_entry() {
    let (env, contract) = deployed();
    let first = Address::generate(&env);
    let second = Address::generate(&env);
    let client = client(&env, &contract);
    client.deposit(&first, &1_000);
    client.deposit(&second, &1_000);

    client.register(&first, &name(&env, "alice"), &1);
    let after_first = StorageSnapshot::capture(&env, &contract);
    client.register(&second, &name(&env, "bob"), &1);
    let after_second = StorageSnapshot::capture(&env, &contract);

    // The registry keeps every name in one map, so a second registration grows
    // the value of `domains` rather than opening a second key. That is the
    // storage-growth shape the benchmark is about: bytes per name, not entries
    // per name.
    let before_growth = after_first.get("domains").unwrap().value.len();
    let after_growth = after_second.get("domains").unwrap().value.len();
    assert!(
        after_growth > before_growth,
        "the registry value should grow by one domain: {before_growth} -> {after_growth}"
    );
    assert_eq!(
        after_first.entries().len(),
        after_second.entries().len(),
        "a second registration should rewrite the map, not add a key"
    );
}

#[test]
fn records_live_beside_the_registry_in_persistent_storage() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    let label = name(&env, "alice");
    let client = client(&env, &contract);
    client.deposit(&owner, &1_000);
    client.register(&owner, &label, &1);

    client.set_record(
        &owner,
        &label,
        &RecordKind::Stellar,
        &name(
            &env,
            "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF",
        ),
    );

    // Records are their own persistent key, and the instance keys the registry
    // was initialized with are untouched by either call.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "records", StorageTier::Persistent);
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "*s", StorageTier::Persistent);
    assert_eq!(
        inspect_storage(&env, &contract)
            .iter()
            .map(|entry| entry.qualified())
            .collect::<Vec<StdString>>(),
        [
            "instance:admin",
            "instance:base_fee",
            "instance:grace_period",
            "instance:treasury",
            "persistent:balances",
            "persistent:domains",
            "persistent:records",
        ]
    );
}

#[test]
fn a_deposit_opens_the_balance_entry_and_nothing_else() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);

    let before = StorageSnapshot::capture(&env, &contract);
    client(&env, &contract).deposit(&holder, &500);
    let after = StorageSnapshot::capture(&env, &contract);

    before.assert_entry_added(&after, "balances");
    soroban_testkit_core::assert_storage_tier!(
        &env,
        &contract,
        "balances",
        StorageTier::Persistent
    );
}

#[test]
fn reading_state_leaves_storage_unchanged() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    let label = name(&env, "alice");
    let client = client(&env, &contract);
    client.deposit(&owner, &1_000);
    client.register(&owner, &label, &1);
    client.set_record(&owner, &label, &RecordKind::Text, &name(&env, "hello"));

    let before = StorageSnapshot::capture(&env, &contract);
    client.owner_of(&label);
    client.domain(&label);
    client.resolve(&label, &RecordKind::Text);
    client.is_available(&label);
    before.assert_unchanged(&StorageSnapshot::capture(&env, &contract));
}

#[test]
#[should_panic(
    expected = "Storage tier mismatch for key 'domains': expected temporary, found persistent"
)]
fn a_key_in_the_wrong_tier_is_a_failure() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    client(&env, &contract).deposit(&owner, &1_000);
    client(&env, &contract).register(&owner, &name(&env, "alice"), &1);

    // The registry is persistent, so a test that expects it to expire is wrong
    // about the contract and says so.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "domains", StorageTier::Temporary);
}

#[test]
fn a_baseline_survives_the_file_ci_commits() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    let label = name(&env, "alice");
    let client = client(&env, &contract);
    client.deposit(&owner, &10_000);

    let mut baseline = BudgetBaseline::new();
    client.register(&owner, &label, &1);
    baseline.record("register", BudgetSnapshot::last_invocation(&env));
    client.renew(&owner, &label, &1);
    baseline.record("renew", BudgetSnapshot::last_invocation(&env));

    // Round-tripping through the serialised form is what lets the Budget
    // baseline workflow compare a pull request against costs recorded on `main`.
    let reloaded = BudgetBaseline::parse(&baseline.to_json_string())
        .expect("a baseline this crate wrote parses");

    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded.get("register"), baseline.get("register"));
    assert_eq!(
        reloaded
            .cases()
            .map(|(case, _)| case)
            .collect::<Vec<&str>>(),
        ["register", "renew"]
    );
}
