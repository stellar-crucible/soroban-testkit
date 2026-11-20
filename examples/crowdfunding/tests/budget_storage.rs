mod common;
use common::Fixture;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::Address;
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_fixtures::TestContext;

#[test]
fn campaign_storage_snapshot_has_persistent_entry() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    let snap = StorageSnapshot::capture(&f.env, &f.id);
    assert!(!snap.entries().is_empty());
}
#[test]
fn contract_call_consumes_budget() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    f.client().contribute(&1, &f.backer, &100);
    assert!(BudgetSnapshot::last_invocation(&f.env).cpu_insns > 0);
}
#[test]
fn fixture_crate_context_is_available() {
    let ctx = TestContext::new();
    assert!(ctx.users.is_empty());
}
#[test]
fn testkit_assert_crate_is_available() {
    let report = soroban_testkit_assert::balance::BalanceConservationReport {
        accounts: vec![],
        pre_total: 0,
        post_total: 0,
        actual_delta: 0,
        mints: 0,
        burns: 0,
        expected_delta: 0,
        discrepancy: 0,
    };
    assert!(report.is_conserved());
}
#[test]
fn batch_refund_gas_baseline_is_bounded() {
    let f = Fixture::new(10000);
    f.open(20000, 100);
    let mut backers = soroban_sdk::Vec::new(&f.env);
    for _ in 0..8 {
        let b = Address::generate(&f.env);
        soroban_sdk::token::StellarAssetClient::new(&f.env, &f.token).mint(&b, &100);
        f.client().contribute(&1, &b, &100);
        backers.push_back(b);
    }
    f.env.ledger().set_timestamp(100);
    f.client().finalize_failure(&1);
    let mut total = 0u64;
    for b in backers {
        f.client().claim_refund(&1, &b);
        total += BudgetSnapshot::last_invocation(&f.env).cpu_insns;
    }
    assert!(total <= 8 * 300_000, "measured batch refund CPU: {total}");
}
