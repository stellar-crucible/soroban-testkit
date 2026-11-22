extern crate std;

use soroban_sdk::{testutils::Address as _, Address, Env};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_vamm::{VammContract, VammContractClient};

fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(VammContract, ());
    let client = VammContractClient::new(&env, &contract);
    client.initialize(&10_000_000, &10_000_000);
    (env, contract)
}

#[test]
fn opening_a_position_stays_inside_budget() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    let trader = Address::generate(&env);
    soroban_testkit_core::budget_guard!(
        &env,
        "open_position",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || client.open_position(&trader, &1, &1_000, &5)
    );
}

#[test]
fn mark_move_stays_inside_budget() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    let trader = Address::generate(&env);
    client.open_position(&trader, &1, &1_000, &3);
    soroban_testkit_core::budget_guard!(
        &env,
        "move_mark",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || client.move_mark(&10_000_000, &12_000_000)
    );
}

#[test]
fn reading_mark_is_cheaper_than_opening() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    client.mark();
    let read = BudgetSnapshot::last_invocation(&env);
    let trader = Address::generate(&env);
    client.open_position(&trader, &1, &1_000, &2);
    let open = BudgetSnapshot::last_invocation(&env);
    assert!(read.cpu_insns <= open.cpu_insns);
}

#[test]
fn position_write_is_persistent_storage() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    let trader = Address::generate(&env);
    client.open_position(&trader, &1, &1_000, &2);
    let snapshot = StorageSnapshot::capture(&env, &contract);
    assert!(!snapshot.in_tier(StorageTier::Persistent).is_empty());
}

#[test]
fn vamm_state_is_instance_storage() {
    let (env, contract) = setup();
    let snapshot = StorageSnapshot::capture(&env, &contract);
    assert!(!snapshot.in_tier(StorageTier::Instance).is_empty());
}

#[test]
fn storage_snapshot_detects_a_new_position() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    let before = StorageSnapshot::capture(&env, &contract);
    let trader = Address::generate(&env);
    client.open_position(&trader, &1, &1_000, &2);
    let after = StorageSnapshot::capture(&env, &contract);
    assert!(!before.diff(&after).added().is_empty());
}

#[test]
fn closing_removes_position_storage() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    let trader = Address::generate(&env);
    client.open_position(&trader, &1, &1_000, &2);
    let before = StorageSnapshot::capture(&env, &contract);
    client.close_position(&trader);
    let after = StorageSnapshot::capture(&env, &contract);
    assert!(client.position(&trader).is_none());
    assert!(!before.diff(&after).modified().is_empty());
}

#[test]
fn baseline_can_record_open_cost() {
    let (env, contract) = setup();
    let client = VammContractClient::new(&env, &contract);
    let trader = Address::generate(&env);
    client.open_position(&trader, &1, &1_000, &2);
    let cost = BudgetSnapshot::last_invocation(&env);
    let mut baseline = BudgetBaseline::new();
    baseline.record("open_position", cost);
    assert_eq!(baseline.get("open_position"), Some(cost));
}
