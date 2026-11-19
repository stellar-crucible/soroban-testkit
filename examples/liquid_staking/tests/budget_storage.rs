mod common;
use common::Fixture;
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_testkit_fixtures::TestContext;

#[test]
fn initialized_pool_is_visible_in_storage_snapshot() {
    let f = Fixture::new(500);
    let s = StorageSnapshot::capture(&f.env, &f.id);
    assert!(!s.is_empty());
}
#[test]
fn deposit_and_exchange_rate_fit_budget() {
    let f = Fixture::new(500);
    f.client().deposit(&f.user, &200);
    let guard = BudgetGuard::new("exchange_rate")
        .cpu_ceiling(100_000)
        .mem_ceiling(20_000);
    let rate = guard.run(&f.env, || f.client().exchange_rate());
    assert_eq!(rate, 1_000_000);
    let measured = BudgetSnapshot::last_invocation(&f.env);
    assert!(measured.cpu_insns <= 100_000);
}
#[test]
fn exchange_rate_has_fixed_precision() {
    let f = Fixture::new(500);
    f.client().deposit(&f.user, &250);
    assert_eq!(f.client().exchange_rate(), 1_000_000);
}
#[test]
fn fixture_test_context_is_usable() {
    let ctx = TestContext::new();
    assert!(ctx.users.is_empty());
}
#[test]
fn assertion_crate_balance_report_is_conserved() {
    let r = soroban_testkit_assert::balance::BalanceConservationReport {
        accounts: vec![],
        pre_total: 0,
        post_total: 0,
        actual_delta: 0,
        mints: 0,
        burns: 0,
        expected_delta: 0,
        discrepancy: 0,
    };
    assert!(r.is_conserved());
}
