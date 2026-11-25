use super::*;
use soroban_sdk::{testutils::Address as _, token::StellarAssetClient, Address, Env};

struct TestEnv {
    env: Env,
    client: EscrowContractClient<'static>,
    token: Address,
    buyer: Address,
    seller: Address,
    arbiter: Address,
}

impl TestEnv {
    fn setup(total_amount: i128, timeout_ledger: u32) -> Self {
        let env = Env::default();
        env.mock_all_auths();

        let buyer = Address::generate(&env);
        let seller = Address::generate(&env);
        let arbiter = Address::generate(&env);
        let token_admin = Address::generate(&env);

        let token = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        StellarAssetClient::new(&env, &token).mint(&buyer, &(total_amount * 2));

        let contract_id = env.register(EscrowContract, ());
        let client = EscrowContractClient::new(&env, &contract_id);

        client.initialize(
            &buyer,
            &seller,
            &arbiter,
            &token,
            &total_amount,
            &timeout_ledger,
        );

        Self {
            env,
            client,
            token,
            buyer,
            seller,
            arbiter,
        }
    }
}

#[test]
fn test_initialize_sets_config() {
    let t = TestEnv::setup(1_000, 100);
    let cfg = t.client.get_config();
    assert_eq!(cfg.buyer, t.buyer);
    assert_eq!(cfg.seller, t.seller);
    assert_eq!(cfg.arbiter, t.arbiter);
    assert_eq!(cfg.total_amount, 1_000);
    assert_eq!(cfg.deposited_amount, 0);
    assert_eq!(cfg.released_amount, 0);
    assert_eq!(cfg.timeout_ledger, 100);
    assert_eq!(cfg.status, EscrowStatus::Active);
}

#[test]
fn test_double_initialization_fails() {
    let t = TestEnv::setup(1_000, 100);
    let res = t
        .client
        .try_initialize(&t.buyer, &t.seller, &t.arbiter, &t.token, &1_000, &100);
    assert_eq!(res, Err(Ok(EscrowError::AlreadyInitialized)));
}

#[test]
fn test_zero_amount_initialization_fails() {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(EscrowContract, ());
    let client = EscrowContractClient::new(&env, &contract_id);
    let res = client.try_initialize(
        &Address::generate(&env),
        &Address::generate(&env),
        &Address::generate(&env),
        &Address::generate(&env),
        &0,
        &100,
    );
    assert_eq!(res, Err(Ok(EscrowError::InvalidAmount)));
}

#[test]
fn test_add_milestones_and_query() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "ui_design"), &400);
    t.client
        .add_milestone(&t.buyer, &2, &Symbol::new(&t.env, "contract"), &600);

    let m1 = t.client.get_milestone(&1).expect("milestone 1 exists");
    assert_eq!(m1.id, 1);
    assert_eq!(m1.amount, 400);
    assert_eq!(m1.status, MilestoneStatus::Pending);

    let m2 = t.client.get_milestone(&2).expect("milestone 2 exists");
    assert_eq!(m2.id, 2);
    assert_eq!(m2.amount, 600);
    assert_eq!(m2.status, MilestoneStatus::Pending);
}

#[test]
fn test_duplicate_milestone_id_fails() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &500);
    let res = t
        .client
        .try_add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &500);
    assert_eq!(res, Err(Ok(EscrowError::AlreadyInitialized)));
}

#[test]
fn test_deposit_updates_deposited_amount() {
    let t = TestEnv::setup(1_000, 100);
    t.client.deposit(&t.buyer, &600);
    assert_eq!(t.client.get_remaining_funds(), 600);

    t.client.deposit(&t.buyer, &400);
    assert_eq!(t.client.get_remaining_funds(), 1_000);
}

#[test]
fn test_deposit_exceeding_total_fails() {
    let t = TestEnv::setup(1_000, 100);
    let res = t.client.try_deposit(&t.buyer, &1_001);
    assert_eq!(res, Err(Ok(EscrowError::ExceedsTotal)));
}

#[test]
fn test_milestone_approval_and_release() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &400);
    t.client.deposit(&t.buyer, &1_000);

    t.client.approve_milestone(&t.buyer, &1);

    let m1 = t.client.get_milestone(&1).unwrap();
    assert_eq!(m1.status, MilestoneStatus::Released);

    let token_client = token::Client::new(&t.env, &t.token);
    assert_eq!(token_client.balance(&t.seller), 400);
    assert_eq!(t.client.get_remaining_funds(), 600);
}

#[test]
fn test_milestone_approval_by_arbiter() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &500);
    t.client.deposit(&t.buyer, &500);

    t.client.approve_milestone(&t.arbiter, &1);

    let token_client = token::Client::new(&t.env, &t.token);
    assert_eq!(token_client.balance(&t.seller), 500);
}

#[test]
fn test_unauthorized_milestone_approval_fails() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &500);
    t.client.deposit(&t.buyer, &500);

    let attacker = Address::generate(&t.env);
    let res = t.client.try_approve_milestone(&attacker, &1);
    assert_eq!(res, Err(Ok(EscrowError::Unauthorized)));
}

#[test]
fn test_approval_with_insufficient_deposit_fails() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &500);
    t.client.deposit(&t.buyer, &200);

    let res = t.client.try_approve_milestone(&t.buyer, &1);
    assert_eq!(res, Err(Ok(EscrowError::InsufficientDeposit)));
}

#[test]
fn test_milestone_already_approved_fails() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &500);
    t.client.deposit(&t.buyer, &500);
    t.client.approve_milestone(&t.buyer, &1);

    let res = t.client.try_approve_milestone(&t.buyer, &1);
    assert_eq!(res, Err(Ok(EscrowError::MilestoneAlreadyApproved)));
}

#[test]
fn test_full_release_completes_escrow() {
    let t = TestEnv::setup(1_000, 100);
    t.client
        .add_milestone(&t.buyer, &1, &Symbol::new(&t.env, "m1"), &1_000);
    t.client.deposit(&t.buyer, &1_000);
    t.client.approve_milestone(&t.buyer, &1);

    assert_eq!(t.client.get_status(), EscrowStatus::Completed);
    assert_eq!(t.client.get_remaining_funds(), 0);
}
