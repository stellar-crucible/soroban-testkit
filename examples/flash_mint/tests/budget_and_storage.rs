use soroban_sdk::{contract, contractimpl, testutils::Address as _, Address, Bytes, Env, Symbol};
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_core::StorageFragmentationAnalyzer;
use soroban_testkit_example_flash_mint::*;

#[contract]
pub struct BudgetBorrower;

#[contractimpl]
impl BudgetBorrower {
    pub fn on_flash_loan(
        _env: Env,
        _initiator: Address,
        _token: Address,
        _amount: i128,
        _fee: i128,
        _data: Bytes,
    ) -> Symbol {
        CALLBACK_SUCCESS
    }
}

fn setup_token(
    env: &Env,
    admin: &Address,
    treasury: &Address,
    fee_bps: u32,
    max_mint: i128,
) -> (Address, FlashMintTokenClient<'static>) {
    let token_id = env.register(FlashMintToken, ());
    let client = FlashMintTokenClient::new(env, &token_id);
    client.initialize(admin, treasury, &fee_bps, &max_mint);
    (token_id, client)
}

#[test]
fn test_budget_metering_for_flash_mint() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 0, 1_000_000);

    let borrower_id = env.register(BudgetBorrower, ());

    // Execute flash loan under a BudgetGuard
    let guard = BudgetGuard::new("flash_loan")
        .cpu_ceiling(5_000_000)
        .mem_ceiling(1_000_000);

    let res = guard.run(&env, || {
        client.flash_loan(&borrower_id, &token_id, &100_000, &Bytes::new(&env))
    });

    assert!(res);

    let cost = BudgetSnapshot::last_invocation(&env);
    assert!(cost.cpu_insns > 0);
    assert!(cost.mem_bytes > 0);
}

#[test]
fn test_storage_snapshot_atomic_invariance() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 0, 1_000_000);

    let borrower_id = env.register(BudgetBorrower, ());

    let before = StorageSnapshot::capture(&env, &token_id);

    client.flash_loan(&borrower_id, &token_id, &50_000, &Bytes::new(&env));

    let after = StorageSnapshot::capture(&env, &token_id);

    // After zero-fee flash mint, storage entries remain unchanged
    before.assert_unchanged(&after);
}

#[test]
fn test_storage_tier_count_quotas_on_flash_mint_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let borrower_id = env.register(BudgetBorrower, ());
    client.mint(&borrower_id, &100);

    // Assert quotas using the new assert_storage_tier_count! macro
    assert_storage_tier_count!(env, token_id, {
        max_instance: 10,
        max_persistent: 5,
        temporary: 0,
    });

    assert_storage_tier_count!(env, token_id, max_instance = 6, persistent = 1);
}

#[test]
fn test_storage_fragmentation_analyzer_on_flash_mint_contract() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let user_a = Address::generate(&env);
    let user_b = Address::generate(&env);

    client.mint(&user_a, &500);
    client.mint(&user_b, &1_000);

    let analyzer = StorageFragmentationAnalyzer::new()
        .with_overhead_threshold(0.5)
        .with_stroop_rate_per_byte(100);

    let report = analyzer.analyze(&env, &token_id);

    assert!(!report.entries.is_empty());
    assert!(report.total_bytes > 0);

    let md = report.to_markdown();
    assert!(md.contains("# Storage Fragmentation & Serialization Overhead Report"));
    assert!(md.contains("## Tier Breakdown"));
}

#[test]
fn test_storage_diff_detects_fee_addition_to_treasury() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 50, 1_000_000);

    let borrower = env.register(BudgetBorrower, ());
    let fee = client.flash_fee(&token_id, &100_000);
    client.mint(&borrower, &fee);

    let before = StorageSnapshot::capture(&env, &token_id);
    client.flash_loan(&borrower, &token_id, &100_000, &Bytes::new(&env));
    let after = StorageSnapshot::capture(&env, &token_id);

    let diff = before.diff(&after);
    assert!(!diff.is_empty());
}

#[test]
#[should_panic(expected = "Storage allocation quota exceeded")]
fn test_storage_quota_breach_detected() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 10, 1_000_000);

    let user = Address::generate(&env);
    client.mint(&user, &100);

    // Expecting 0 persistent entries, but 1 was added
    assert_storage_tier_count!(env, token_id, {
        persistent: 0,
    });
}

#[test]
fn test_budget_guard_with_tight_limits() {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let treasury = Address::generate(&env);
    let (token_id, client) = setup_token(&env, &admin, &treasury, 0, 1_000_000);

    let borrower = env.register(BudgetBorrower, ());

    let guard = BudgetGuard::new("flash_loan_tight")
        .cpu_ceiling(10_000_000)
        .mem_ceiling(5_000_000);

    let outcome = guard.run(&env, || {
        client.flash_loan(&borrower, &token_id, &10_000, &Bytes::new(&env))
    });
    assert!(outcome);
}
