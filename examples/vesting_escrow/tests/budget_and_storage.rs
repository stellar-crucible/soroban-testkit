//! One crate per file: this suite drives `examples/vesting_escrow` through
//! `soroban-testkit-core`, benchmarking resource overhead and storage tiers.

use std::path::{Path, PathBuf};

use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::{Address, Env};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_vesting_escrow::{VestingEscrow, VestingEscrowClient};

struct Proto {
    env: Env,
    contract_id: Address,
    token: Address,
    admin: Address,
    beneficiary: Address,
}

fn deploy() -> Proto {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let admin = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    let contract_id = env.register(VestingEscrow, ());
    let client = VestingEscrowClient::new(&env, &contract_id);

    let token_admin = Address::generate(&env);
    let token_id = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();
    let asset = StellarAssetClient::new(&env, &token_id);
    asset.mint(&admin, &1_000_000);

    client.create_schedule(
        &admin,
        &beneficiary,
        &token_id,
        &100_000,
        &1_700_000_000,
        &0,
        &100,
        &true,
    );

    Proto {
        env,
        contract_id,
        token: token_id,
        admin,
        beneficiary,
    }
}

fn client<'a>(proto: &'a Proto) -> VestingEscrowClient<'a> {
    VestingEscrowClient::new(&proto.env, &proto.contract_id)
}

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

#[test]
fn create_stays_inside_its_transaction_ceiling() {
    let proto = deploy();
    let admin = proto.admin.clone();
    let beneficiary = proto.beneficiary.clone();
    let token = proto.token.clone();

    // Need a fresh protocol for create measurement.
    let env2 = Env::default();
    env2.mock_all_auths();
    env2.ledger().set_timestamp(1_700_000_000);
    let admin2 = Address::generate(&env2);
    let beneficiary2 = Address::generate(&env2);
    let contract_id2 = env2.register(VestingEscrow, ());
    let client2 = VestingEscrowClient::new(&env2, &contract_id2);
    let token_admin2 = Address::generate(&env2);
    let token_id2 = env2
        .register_stellar_asset_contract_v2(token_admin2)
        .address();
    StellarAssetClient::new(&env2, &token_id2).mint(&admin2, &1_000_000);

    soroban_testkit_core::budget_guard!(
        &env2,
        "create_schedule",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || client2.create_schedule(&admin2, &beneficiary2, &token_id2, &100_000, &1_700_000_000, &0, &100, &true)
    );
    let _ = (admin, beneficiary, token);
}

#[test]
fn claim_stays_inside_its_transaction_ceiling() {
    let proto = deploy();
    let env = &proto.env;
    let c = client(&proto);
    env.ledger().set_timestamp(1_700_000_025);

    soroban_testkit_core::budget_guard!(
        &proto.env,
        "claim",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || c.claim(&0)
    );
}

#[test]
fn revoke_stays_inside_its_transaction_ceiling() {
    let proto = deploy();
    let env = &proto.env;
    let c = client(&proto);
    env.ledger().set_timestamp(1_700_000_025);
    c.claim(&0);
    env.ledger().set_timestamp(1_700_000_025);

    soroban_testkit_core::budget_guard!(
        &proto.env,
        "revoke",
        { cpu_max: 50_000_000, mem_max: 50_000_000 },
        || c.revoke(&0)
    );
}

#[test]
fn read_costs_less_cpu_than_claim() {
    let proto = deploy();
    let env = &proto.env;
    let c = client(&proto);
    env.ledger().set_timestamp(1_700_000_025);
    c.claim(&0);
    let claimed = BudgetSnapshot::last_invocation(env);

    c.vested(&0);
    let read = BudgetSnapshot::last_invocation(env);

    assert!(read.cpu_insns < claimed.cpu_insns);
}

#[test]
fn initialize_populates_instance_storage_only() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);
    let contract_id = env.register(VestingEscrow, ());
    let _ = VestingEscrowClient::new(&env, &contract_id);

    let snapshot = StorageSnapshot::capture(&env, &contract_id);
    assert!(snapshot.in_tier(StorageTier::Persistent).is_empty());
    assert!(snapshot.in_tier(StorageTier::Temporary).is_empty());
}

#[test]
fn schedule_lands_in_persistent_tier() {
    let proto = deploy();
    let env = &proto.env;

    soroban_testkit_core::assert_storage_tier!(
        &env,
        &proto.contract_id,
        "sched",
        StorageTier::Persistent
    );

    let snapshot = StorageSnapshot::capture(env, &proto.contract_id);
    let persistent_keys: Vec<String> = snapshot
        .in_tier(StorageTier::Persistent)
        .iter()
        .map(|e| e.key.clone())
        .collect();
    assert!(persistent_keys.contains(&"sched".to_string()));
}

#[test]
fn committed_budget_baseline_loads_and_covers_lifecycle() {
    let committed = BudgetBaseline::load(&baseline_path())
        .expect("examples/vesting_escrow/budget.json must exist and parse");

    for case in ["create_schedule", "claim", "revoke"] {
        assert!(committed.get(case).is_some(), "missing baseline for {case}");
        let snap = committed.get(case).unwrap();
        assert!(snap.cpu_insns > 0 && snap.mem_bytes > 0);
    }

    let round_tripped = BudgetBaseline::parse(&committed.to_json_string())
        .expect("serialized baseline must parse cleanly");
    assert_eq!(round_tripped, committed);
}

#[test]
#[ignore = "rewrites examples/vesting_escrow/budget.json — run deliberately"]
fn budget_baseline_records_current_costs() {
    let mut baseline = BudgetBaseline::new();

    // create
    {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_700_000_000);
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register(VestingEscrow, ());
        let client = VestingEscrowClient::new(&env, &contract_id);
        let token_admin = Address::generate(&env);
        let token_id = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        StellarAssetClient::new(&env, &token_id).mint(&admin, &1_000_000);
        client.create_schedule(
            &admin,
            &beneficiary,
            &token_id,
            &100_000,
            &1_700_000_000,
            &0,
            &100,
            &true,
        );
        baseline.record("create_schedule", BudgetSnapshot::last_invocation(&env));
    }
    // claim
    {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_700_000_000);
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register(VestingEscrow, ());
        let client = VestingEscrowClient::new(&env, &contract_id);
        let token_admin = Address::generate(&env);
        let token_id = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        StellarAssetClient::new(&env, &token_id).mint(&admin, &1_000_000);
        client.create_schedule(
            &admin,
            &beneficiary,
            &token_id,
            &100_000,
            &1_700_000_000,
            &0,
            &100,
            &true,
        );
        env.ledger().set_timestamp(1_700_000_025);
        client.claim(&0);
        baseline.record("claim", BudgetSnapshot::last_invocation(&env));
    }
    // revoke
    {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().set_timestamp(1_700_000_000);
        let admin = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register(VestingEscrow, ());
        let client = VestingEscrowClient::new(&env, &contract_id);
        let token_admin = Address::generate(&env);
        let token_id = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        StellarAssetClient::new(&env, &token_id).mint(&admin, &1_000_000);
        client.create_schedule(
            &admin,
            &beneficiary,
            &token_id,
            &100_000,
            &1_700_000_000,
            &0,
            &100,
            &true,
        );
        env.ledger().set_timestamp(1_700_000_025);
        client.claim(&0);
        env.ledger().set_timestamp(1_700_000_025);
        client.revoke(&0);
        baseline.record("revoke", BudgetSnapshot::last_invocation(&env));
    }

    baseline.save(&baseline_path()).unwrap();
    println!("{}", baseline.to_json_string());
}
