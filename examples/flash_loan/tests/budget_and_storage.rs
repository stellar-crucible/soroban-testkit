//! soroban-testkit-core: CPU and memory of flash loans checked against the
//! committed `budget.json`, hard ceilings, and what a loan does (and does not
//! do) to storage.

mod common;

use std::path::{Path, PathBuf};

use common::Market;
use soroban_sdk::vec;
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::budget::{BudgetBaseline, BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_flash_loan::RepayMode;

/// Growth a committed baseline tolerates before the suite fails.
const BASELINE_TOLERANCE_PERCENT: u64 = 10;

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("budget.json")
}

fn single_loan_cost() -> BudgetSnapshot {
    let market = Market::setup();
    market.borrow(0, 1_000_000);
    BudgetSnapshot::last_invocation(&market.env)
}

fn multi_loan_cost() -> BudgetSnapshot {
    let market = Market::setup();
    let env = &market.env;
    market.lender().flash_loan_multi(
        &market.borrower,
        &market.receiver,
        &vec![env, market.tokens[0].clone(), market.tokens[1].clone()],
        &vec![env, 1_000_000i128, 2_000_000i128],
        &market.data(),
    );
    BudgetSnapshot::last_invocation(env)
}

fn measured() -> [(&'static str, BudgetSnapshot); 2] {
    [
        ("flash_loan_single", single_loan_cost()),
        ("flash_loan_multi_2_tokens", multi_loan_cost()),
    ]
}

#[test]
fn flash_loan_costs_stay_within_the_committed_baseline() {
    let committed = BudgetBaseline::load(&baseline_path())
        .expect("examples/flash_loan/budget.json must exist and parse");
    for (case, cost) in measured() {
        assert!(committed.get(case).is_some(), "missing baseline for {case}");
        committed
            .guard(case)
            .tolerance_percent(BASELINE_TOLERANCE_PERCENT)
            .assert_within(&cost);
    }
}

#[test]
fn a_single_loan_stays_under_hard_ceilings() {
    let market = Market::setup();
    let guard = BudgetGuard::new("flash_loan")
        .cpu_ceiling(20_000_000)
        .mem_ceiling(5_000_000);

    let fee = guard.run(&market.env, || market.borrow(0, 1_000_000));

    assert_eq!(fee, 900);
    let cost = BudgetSnapshot::last_invocation(&market.env);
    assert!(cost.cpu_insns > 0);
    assert!(cost.mem_bytes > 0);
}

#[test]
fn a_second_token_costs_more_but_less_than_a_second_loan() {
    let single = single_loan_cost();
    let multi = multi_loan_cost();

    assert!(multi.cpu_insns > single.cpu_insns);
    assert!(multi.cpu_insns < single.cpu_insns * 2);
}

#[test]
fn a_repaid_loan_records_fees_in_persistent_storage() {
    let market = Market::setup();
    let before = StorageSnapshot::capture(&market.env, &market.provider);

    market.borrow(0, 1_000_000);

    let after = StorageSnapshot::capture(&market.env, &market.provider);
    let diff = before.diff(&after);
    let added_in = |tier: StorageTier| diff.added().iter().filter(|e| e.tier == tier).count();
    // FeesCollected(token) and BorrowerFees(borrower, token).
    assert_eq!(added_in(StorageTier::Persistent), 2);
    // LoanCount.
    assert_eq!(added_in(StorageTier::Instance), 1);
    assert!(diff.removed().is_empty());
}

#[test]
fn a_failed_loan_leaves_provider_storage_untouched() {
    let market = Market::setup();
    market.set_mode(RepayMode::ShortBy(1));
    let before = StorageSnapshot::capture(&market.env, &market.provider);

    let outcome = market.lender().try_flash_loan(
        &market.borrower,
        &market.receiver,
        &market.tokens[0],
        &1_000_000,
        &market.data(),
    );

    assert!(outcome.is_err());
    before.assert_unchanged(&StorageSnapshot::capture(&market.env, &market.provider));
}

#[test]
fn provider_storage_stays_small_and_temporary_free() {
    let market = Market::setup();
    market.borrow(0, 1_000);
    market.borrow(1, 1_000);

    // Supported(t0), Supported(t1), and per token FeesCollected and
    // BorrowerFees for the one borrower.
    assert_storage_tier_count!(market.env, market.provider, {
        max_instance: 3,
        max_persistent: 6,
        temporary: 0,
    });
}

#[test]
fn repeated_loans_reuse_the_same_fee_entries() {
    let market = Market::setup();
    market.borrow(0, 1_000);
    let before = StorageSnapshot::capture(&market.env, &market.provider);

    market.borrow(0, 2_000);

    let diff = before.diff(&StorageSnapshot::capture(&market.env, &market.provider));
    assert!(
        diff.added().is_empty(),
        "no new entries for a returning borrower"
    );
    // FeesCollected, BorrowerFees and LoanCount are rewritten in place.
    assert_eq!(diff.modified().len(), 3);
}

#[test]
#[ignore = "rewrites examples/flash_loan/budget.json — run deliberately"]
fn budget_baseline_records_current_costs() {
    let mut baseline = BudgetBaseline::new();
    for (case, cost) in measured() {
        baseline.record(case, cost);
    }
    baseline.save(&baseline_path()).unwrap();
    println!("{}", baseline.to_json_string());
}
