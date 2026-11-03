//! One crate per file: this suite drives the token example through
//! `soroban-testkit-core`, whose two jobs here are naming what a call cost and
//! which storage tier a key was written to.
//!
//! The harness is a bare `Env` rather than a `TestContext`, so nothing in these
//! tests reaches for another testkit crate — `src/test.rs` is where the fixture
//! builder is demonstrated.

extern crate std;

use std::{string::String, vec::Vec};

use soroban_sdk::{testutils::Address as _, Address, Env};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetMetric, BudgetSnapshot};
use soroban_testkit_core::storage::{inspect_storage, StorageSnapshot, StorageTier};
use soroban_testkit_example_token::{TokenContract, TokenContractClient};

fn deployed() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(TokenContract, ());
    TokenContractClient::new(&env, &contract).initialize(&Address::generate(&env));
    (env, contract)
}

fn client<'a>(env: &'a Env, contract: &Address) -> TokenContractClient<'a> {
    TokenContractClient::new(env, contract)
}

#[test]
fn mint_stays_inside_its_transaction_ceiling() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);

    // A ceiling is transaction headroom, not this call's measured cost. A
    // contract registered natively is metered without the VM's own costs, so the
    // mint below reads tens of thousands of instructions and a network would
    // bill more.
    soroban_testkit_core::budget_guard!(
        &env,
        "mint",
        { cpu_max: 20_000_000, mem_max: 20_000_000 },
        || client(&env, &contract).mint(&holder, &1_000)
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=mint metric=cpu_insns")]
fn a_call_past_its_ceiling_fails_the_test_with_a_parseable_line() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);

    soroban_testkit_core::budget_guard!(&env, "mint", { cpu_max: 1 }, || {
        client(&env, &contract).mint(&holder, &1_000)
    });
}

#[test]
fn the_reading_belongs_to_the_call_that_just_happened() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);
    let client = client(&env, &contract);

    client.mint(&holder, &1_000);
    let minted = BudgetSnapshot::last_invocation(&env);

    client.balance_of(&holder);
    let read = BudgetSnapshot::last_invocation(&env);

    // Two invocations, two readings: `last_invocation` reports the call that has
    // already happened, so the read is measured on its own and not on top of the
    // mint before it.
    assert!(
        read.cpu_insns < minted.cpu_insns,
        "a read that writes nothing should cost less than the mint that does: {} vs {}",
        read.cpu_insns,
        minted.cpu_insns
    );

    // `diff` is the size of the gap between two costs, which is what a change to
    // one code path has to be measured against.
    assert!(read.diff(&minted).cpu_insns > 0);
}

#[test]
fn a_recorded_baseline_accepts_its_own_cost_and_flags_growth() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);

    client(&env, &contract).mint(&holder, &1_000);
    let measured = BudgetSnapshot::last_invocation(&env);

    let mut baseline = BudgetBaseline::new();
    baseline.record("mint", measured);

    // A guard taken from the baseline carries its recorded cost, so the same
    // call again sits inside its own tolerance.
    baseline.guard("mint").tolerance_percent(25).run(&env, || {
        client(&env, &contract).mint(&holder, &2_000);
    });

    // A baseline recorded from a build half as expensive makes this call growth.
    let halved = BudgetSnapshot {
        cpu_insns: measured.cpu_insns / 2,
        mem_bytes: measured.mem_bytes / 2,
    };
    let violations = baseline
        .guard("mint")
        .baseline(Some(halved))
        .tolerance_percent(10)
        .violations(&measured);
    assert!(
        violations
            .iter()
            .any(|violation| violation.metric == BudgetMetric::Cpu),
        "a call costing twice its baseline must be reported as growth"
    );
}

#[test]
fn a_baseline_survives_the_file_ci_commits() {
    let mut baseline = BudgetBaseline::new();
    baseline.record(
        "mint",
        BudgetSnapshot {
            cpu_insns: 41_000,
            mem_bytes: 9_000,
        },
    );
    baseline.record(
        "transfer",
        BudgetSnapshot {
            cpu_insns: 63_000,
            mem_bytes: 12_000,
        },
    );

    // Round-tripping through the serialised form is what lets the Budget
    // baseline workflow compare a pull request against costs recorded on `main`.
    let reloaded = BudgetBaseline::parse(&baseline.to_json_string())
        .expect("a baseline this crate wrote parses");

    assert_eq!(reloaded.len(), 2);
    assert_eq!(reloaded.get("mint"), baseline.get("mint"));
    assert_eq!(
        reloaded
            .cases()
            .map(|(case, _)| case)
            .collect::<Vec<&str>>(),
        ["mint", "transfer"]
    );
}

#[test]
fn initialize_writes_only_instance_storage() {
    let (env, contract) = deployed();

    soroban_testkit_core::assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "supply", StorageTier::Instance);

    let snapshot = StorageSnapshot::capture(&env, &contract);
    assert_eq!(snapshot.len(), 2);
    assert!(snapshot.in_tier(StorageTier::Persistent).is_empty());
}

#[test]
fn the_first_mint_opens_persistent_storage() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);

    let before = StorageSnapshot::capture(&env, &contract);
    client(&env, &contract).mint(&holder, &500);
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
fn balances_and_allowances_share_a_tier_and_a_pattern() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    let spender = Address::generate(&env);
    let client = client(&env, &contract);

    client.mint(&owner, &500);
    client.approve(&owner, &spender, &100);

    // Both maps are keyed by an holder-facing name ending in the same letters,
    // so one suffix pattern covers the pair.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "*es", StorageTier::Persistent);

    let snapshot = StorageSnapshot::capture(&env, &contract);
    assert_eq!(
        snapshot
            .in_tier(StorageTier::Persistent)
            .iter()
            .map(|entry| entry.key.clone())
            .collect::<Vec<String>>(),
        ["allowances", "balances"]
    );
    assert_eq!(
        snapshot
            .in_tier(StorageTier::Instance)
            .iter()
            .map(|entry| entry.key.clone())
            .collect::<Vec<String>>(),
        ["admin", "supply"]
    );
}

#[test]
fn every_key_a_token_holds_reads_back_qualified() {
    let (env, contract) = deployed();
    let owner = Address::generate(&env);
    let spender = Address::generate(&env);
    let client = client(&env, &contract);

    client.mint(&owner, &500);
    client.approve(&owner, &spender, &100);

    // Tier first, then key: the snapshot is captured through an ordered index,
    // so this is the listing a failure message would quote.
    assert_eq!(
        inspect_storage(&env, &contract)
            .iter()
            .map(|entry| entry.qualified())
            .collect::<Vec<String>>(),
        [
            "instance:admin",
            "instance:supply",
            "persistent:allowances",
            "persistent:balances"
        ]
    );
}

#[test]
fn reading_state_leaves_storage_unchanged() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);
    let client = client(&env, &contract);

    client.mint(&holder, &500);
    let before = StorageSnapshot::capture(&env, &contract);

    client.balance_of(&holder);
    client.total_supply();
    before.assert_unchanged(&StorageSnapshot::capture(&env, &contract));
}

#[test]
#[should_panic(
    expected = "Storage tier mismatch for key 'balances': expected temporary, found persistent"
)]
fn a_key_in_the_wrong_tier_is_a_failure() {
    let (env, contract) = deployed();
    let holder = Address::generate(&env);

    client(&env, &contract).mint(&holder, &500);

    // The balances map is persistent, so a test that expects it to expire is
    // wrong about the contract and says so.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "balances", StorageTier::Temporary);
}
