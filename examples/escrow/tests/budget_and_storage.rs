extern crate std;

use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env, Symbol};
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_escrow::{EscrowContract, EscrowContractClient, EscrowStatus};

struct CoreFixture {
    env: Env,
    contract: Address,
    token: Address,
    buyer: Address,
    seller: Address,
    arbiter: Address,
}

impl CoreFixture {
    fn setup() -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let contract = env.register(EscrowContract, ());
        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let arbiter = Address::generate(&env);

        let token = env
            .register_stellar_asset_contract_v2(Address::generate(&env))
            .address();
        StellarAssetClient::new(&env, &token).mint(&buyer, &100_000);

        Self {
            env,
            contract,
            token,
            buyer,
            seller,
            arbiter,
        }
    }

    fn client(&self) -> EscrowContractClient<'_> {
        EscrowContractClient::new(&self.env, &self.contract)
    }

    fn storage(&self) -> StorageSnapshot {
        StorageSnapshot::capture(&self.env, &self.contract)
    }
}

#[test]
fn test_initialization_budget_and_instance_storage() {
    let f = CoreFixture::setup();
    let client = f.client();

    client.initialize(&f.buyer, &f.seller, &f.arbiter, &f.token, &10_000, &500);

    let budget = BudgetSnapshot::last_invocation(&f.env);
    assert!(budget.cpu_insns > 0, "CPU instructions must be metered");
    assert!(budget.mem_bytes > 0, "Memory bytes must be metered");

    let storage = f.storage();
    assert!(!storage.is_empty());
}

#[test]
fn test_milestone_storage_is_persistent() {
    let f = CoreFixture::setup();
    let client = f.client();

    client.initialize(&f.buyer, &f.seller, &f.arbiter, &f.token, &10_000, &500);
    client.add_milestone(&f.buyer, &1, &Symbol::new(&f.env, "m1"), &5_000);

    let storage = f.storage();
    let persistent_entries = storage.in_tier(StorageTier::Persistent);
    assert_eq!(
        persistent_entries.len(),
        1,
        "Milestone must be stored in persistent tier"
    );
}

#[test]
fn test_deposit_budget_metering() {
    let f = CoreFixture::setup();
    let client = f.client();

    client.initialize(&f.buyer, &f.seller, &f.arbiter, &f.token, &10_000, &500);
    client.deposit(&f.buyer, &5_000);

    let budget = BudgetSnapshot::last_invocation(&f.env);
    assert!(
        budget.cpu_insns > 10_000,
        "Deposit involves token transfer and meters instructions"
    );
    assert!(
        budget.mem_bytes > 1_000,
        "Deposit allocates memory for token contract invocation"
    );
}

#[test]
fn test_approve_milestone_budget_metering() {
    let f = CoreFixture::setup();
    let client = f.client();

    client.initialize(&f.buyer, &f.seller, &f.arbiter, &f.token, &10_000, &500);
    client.add_milestone(&f.buyer, &1, &Symbol::new(&f.env, "m1"), &5_000);
    client.deposit(&f.buyer, &5_000);

    client.approve_milestone(&f.buyer, &1);

    let budget = BudgetSnapshot::last_invocation(&f.env);
    assert!(budget.cpu_insns > 10_000);
    assert!(budget.mem_bytes > 1_000);
}

#[test]
fn test_dispute_resolution_budget_metering() {
    let f = CoreFixture::setup();
    let client = f.client();

    client.initialize(&f.buyer, &f.seller, &f.arbiter, &f.token, &10_000, &500);
    client.deposit(&f.buyer, &10_000);
    client.raise_dispute(&f.buyer, &Symbol::new(&f.env, "defect"));

    client.resolve_dispute(&f.arbiter, &6_000, &4_000);

    let budget = BudgetSnapshot::last_invocation(&f.env);
    assert!(budget.cpu_insns > 20_000);
}

#[test]
fn test_storage_footprint_after_lifecycle() {
    let f = CoreFixture::setup();
    let client = f.client();

    client.initialize(&f.buyer, &f.seller, &f.arbiter, &f.token, &10_000, &500);
    client.add_milestone(&f.buyer, &1, &Symbol::new(&f.env, "m1"), &10_000);
    client.deposit(&f.buyer, &10_000);
    client.approve_milestone(&f.buyer, &1);

    let storage = f.storage();
    assert_eq!(client.get_status(), EscrowStatus::Completed);
    assert_eq!(storage.in_tier(StorageTier::Persistent).len(), 1);
}
