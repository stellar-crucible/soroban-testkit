//! Pool lifecycle driven through `soroban-testkit-fixtures`: underwriting and
//! share pricing, premium accrual, claim filing against live policies, assessor
//! voting, and settlement capped by the reserves.

mod common;

use common::*;
use soroban_sdk::Vec;
use soroban_testkit_example_insurance::{
    shares_for_deposit, ClaimStatus, InsuranceConfig, InsuranceError,
};

#[test]
fn initialize_records_the_panel_and_parameters() {
    let insurance = Insurance::new();

    let config = insurance.pool().config();
    assert_eq!(config.capital_token, insurance.token);
    assert_eq!(config.assessors.len(), 3);
    assert_eq!(config.quorum, QUORUM);
    assert_eq!(config.premium_rate_bps, RATE_BPS);
    assert_eq!(config.max_coverage, MAX_COVERAGE);
    assert_eq!(insurance.pool().reserves(), 0);
    assert_eq!(insurance.pool().total_shares(), 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let insurance = Insurance::new();

    let err = insurance
        .pool()
        .try_initialize(&insurance.config(RATE_BPS, QUORUM, MAX_COVERAGE));
    assert_eq!(
        err.unwrap_err().unwrap(),
        InsuranceError::AlreadyInitialized
    );
}

#[test]
fn initialize_rejects_a_panel_or_a_parameter_it_cannot_run_with() {
    let insurance = Insurance::deployed();
    let env = &insurance.ctx.env;
    let valid = insurance.config(RATE_BPS, QUORUM, MAX_COVERAGE);

    let empty_panel = InsuranceConfig {
        assessors: Vec::new(env),
        ..valid.clone()
    };
    let zero_quorum = insurance.config(RATE_BPS, 0, MAX_COVERAGE);
    let quorum_above_panel = insurance.config(RATE_BPS, 4, MAX_COVERAGE);
    let rate_above_100_percent = insurance.config(10_001, QUORUM, MAX_COVERAGE);
    let no_coverage_cap = insurance.config(RATE_BPS, QUORUM, 0);

    for config in [
        empty_panel,
        zero_quorum,
        quorum_above_panel,
        rate_above_100_percent,
        no_coverage_cap,
    ] {
        let err = insurance.pool().try_initialize(&config);
        assert_eq!(err.unwrap_err().unwrap(), InsuranceError::InvalidParameter);
    }
}

#[test]
fn an_uninitialized_pool_rejects_deposits() {
    let insurance = Insurance::deployed();

    let err = insurance
        .pool()
        .try_deposit(&insurance.provider, &(1_000 * ONE));
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::NotInitialized);
}

#[test]
fn a_non_positive_deposit_is_rejected() {
    let insurance = Insurance::new();

    for amount in [0, -ONE] {
        let err = insurance.pool().try_deposit(&insurance.provider, &amount);
        assert_eq!(err.unwrap_err().unwrap(), InsuranceError::InvalidAmount);
    }
}

#[test]
fn deposit_mints_shares_one_for_one_and_moves_the_capital() {
    let insurance = Insurance::new();

    let shares = insurance.deposit(&insurance.provider, 1_000_000 * ONE);

    assert_eq!(shares, 1_000_000 * ONE);
    assert_eq!(insurance.pool().shares_of(&insurance.provider), shares);
    assert_eq!(insurance.pool().total_shares(), shares);
    assert_eq!(insurance.pool().reserves(), 1_000_000 * ONE);
    assert_eq!(insurance.pool().total_deposited(), 1_000_000 * ONE);
    assert_eq!(
        insurance.token().balance(&insurance.pool_id),
        1_000_000 * ONE
    );
    assert_eq!(
        insurance.token().balance(&insurance.provider),
        FUNDING - 1_000_000 * ONE
    );
}

#[test]
fn a_later_provider_buys_shares_at_the_current_share_price() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    // Premiums belong to the pool, so the share price is above one unit a
    // share by the time the second provider arrives.
    insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    let reserves = insurance.pool().reserves();
    let shares_before = insurance.pool().total_shares();

    let shares = insurance.deposit(&insurance.provider2, 1_000_000 * ONE);

    let expected = shares_for_deposit(1_000_000 * ONE, shares_before, reserves).unwrap();
    assert_eq!(shares, expected);
    assert!(shares < 1_000_000 * ONE);
    assert_eq!(insurance.pool().total_shares(), shares_before + shares);
    assert_eq!(insurance.pool().reserves(), reserves + 1_000_000 * ONE);
}

#[test]
fn withdrawing_every_share_returns_the_reserves() {
    let insurance = Insurance::new();
    let shares = insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let before = insurance.token().balance(&insurance.provider);

    let amount = insurance.pool().withdraw(&insurance.provider, &shares);

    assert_eq!(amount, 1_000_000 * ONE);
    assert_eq!(
        insurance.token().balance(&insurance.provider),
        before + amount
    );
    assert_eq!(insurance.pool().shares_of(&insurance.provider), 0);
    assert_eq!(insurance.pool().total_shares(), 0);
    assert_eq!(insurance.pool().reserves(), 0);
    assert_eq!(insurance.pool().total_withdrawn(), amount);
}

#[test]
fn withdrawing_more_shares_than_held_is_rejected() {
    let insurance = Insurance::new();
    let shares = insurance.deposit(&insurance.provider, 1_000 * ONE);

    let err = insurance
        .pool()
        .try_withdraw(&insurance.provider, &(shares + 1));
    assert_eq!(
        err.unwrap_err().unwrap(),
        InsuranceError::InsufficientShares
    );
    assert_eq!(insurance.pool().shares_of(&insurance.provider), shares);
}

#[test]
fn premium_payment_accrues_into_the_reserves() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let before = insurance.pool().reserves();

    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);

    assert_eq!(policy.id, 1);
    assert_eq!(policy.holder, insurance.holder);
    assert_eq!(policy.coverage, 10_000 * ONE);
    assert_eq!(policy.premium_paid, 500 * ONE);
    assert_eq!(policy.starts_at, insurance.now());
    assert_eq!(policy.expires_at, insurance.now() + YEAR);
    assert_eq!(policy.claim_id, 0);
    assert_eq!(insurance.pool().reserves(), before + 500 * ONE);
    assert_eq!(insurance.pool().total_premiums(), 500 * ONE);
    assert_eq!(
        insurance.token().balance(&insurance.holder),
        FUNDING - 500 * ONE
    );
}

#[test]
fn half_a_year_of_coverage_costs_half_the_annual_rate() {
    let insurance = Insurance::new();

    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR / 2);

    assert_eq!(policy.premium_paid, 250 * ONE);
    assert_eq!(
        policy.premium_paid,
        insurance.premium(10_000 * ONE, YEAR / 2)
    );
}

#[test]
fn a_policy_outside_its_bounds_is_rejected() {
    let insurance = Insurance::new();

    for (coverage, duration) in [
        (0, YEAR),
        (-ONE, YEAR),
        (MAX_COVERAGE + ONE, YEAR),
        (10_000 * ONE, 0),
    ] {
        let err = insurance
            .pool()
            .try_buy_policy(&insurance.holder, &coverage, &duration);
        assert_eq!(err.unwrap_err().unwrap(), InsuranceError::InvalidParameter);
    }
}

#[test]
fn filing_a_claim_records_a_pending_claim() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);

    let claim = insurance.file(&insurance.holder, policy.id, 8_000 * ONE);

    assert_eq!(claim.id, 1);
    assert_eq!(claim.policy_id, policy.id);
    assert_eq!(claim.holder, insurance.holder);
    assert_eq!(claim.loss, 8_000 * ONE);
    assert_eq!(claim.votes_for, 0);
    assert_eq!(claim.votes_against, 0);
    assert_eq!(claim.status, ClaimStatus::Pending);
    assert_eq!(claim.payout, 0);
    assert_eq!(insurance.pool().policy(&policy.id).claim_id, claim.id);
}

#[test]
fn a_loss_above_the_coverage_is_rejected() {
    let insurance = Insurance::new();
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);

    let err = insurance
        .pool()
        .try_file_claim(&insurance.holder, &policy.id, &(10_000 * ONE + 1));
    assert_eq!(
        err.unwrap_err().unwrap(),
        InsuranceError::ClaimExceedsCoverage
    );
}

#[test]
fn a_second_claim_on_the_same_policy_is_rejected() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    insurance.file(&insurance.holder, policy.id, 1_000 * ONE);

    let err = insurance
        .pool()
        .try_file_claim(&insurance.holder, &policy.id, &(2_000 * ONE));
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::ClaimAlreadyFiled);
}

#[test]
fn filing_against_someone_elses_policy_is_rejected() {
    let insurance = Insurance::new();
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);

    let err = insurance
        .pool()
        .try_file_claim(&insurance.holder2, &policy.id, &(1_000 * ONE));
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::PolicyNotFound);

    let err = insurance
        .pool()
        .try_file_claim(&insurance.holder, &99, &(1_000 * ONE));
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::PolicyNotFound);
}

#[test]
fn an_expired_policy_cannot_be_claimed() {
    let mut insurance = Insurance::new();
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    insurance.advance_time(YEAR + 1);

    let err = insurance
        .pool()
        .try_file_claim(&insurance.holder, &policy.id, &(1_000 * ONE));
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::PolicyNotActive);
}

#[test]
fn assessor_votes_resolve_the_claim_at_quorum() {
    let insurance = Insurance::new();
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);

    let first = insurance.cast(&insurance.assessors[0], claim.id, true);
    assert_eq!(first.status, ClaimStatus::Pending);
    assert_eq!(first.votes_for, 1);

    let second = insurance.cast(&insurance.assessors[1], claim.id, true);
    assert_eq!(second.status, ClaimStatus::Approved);
    assert_eq!(second.votes_for, 2);
    assert_eq!(second.votes_against, 0);

    let err = insurance
        .pool()
        .try_vote(&insurance.assessors[2], &claim.id, &true);
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::ClaimNotPending);
}

#[test]
fn only_the_panel_votes_and_only_once_each() {
    let insurance = Insurance::new();
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);

    let err = insurance
        .pool()
        .try_vote(&insurance.outsider, &claim.id, &true);
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::NotAssessor);

    insurance.cast(&insurance.assessors[0], claim.id, true);
    let err = insurance
        .pool()
        .try_vote(&insurance.assessors[0], &claim.id, &false);
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::AlreadyVoted);
    assert!(insurance
        .pool()
        .has_voted(&claim.id, &insurance.assessors[0]));
    assert!(!insurance
        .pool()
        .has_voted(&claim.id, &insurance.assessors[1]));
}

#[test]
fn a_tie_rejects_the_claim() {
    let insurance = Insurance::new();
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);

    insurance.cast(&insurance.assessors[0], claim.id, true);
    let resolved = insurance.cast(&insurance.assessors[1], claim.id, false);

    assert_eq!(resolved.status, ClaimStatus::Rejected);
    assert_eq!(resolved.votes_for, 1);
    assert_eq!(resolved.votes_against, 1);
    assert_eq!(insurance.pool().total_payouts(), 0);
}

#[test]
fn settling_an_approved_claim_pays_the_loss() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let (policy, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.approve(claim.id);
    let holder_before = insurance.token().balance(&insurance.holder);
    let reserves_before = insurance.pool().reserves();

    let settled = insurance.settle(claim.id);

    assert_eq!(settled.status, ClaimStatus::Settled);
    assert_eq!(settled.payout, 8_000 * ONE);
    assert_eq!(settled.loss, 8_000 * ONE);
    assert_eq!(settled.policy_id, policy.id);
    assert_eq!(
        insurance.token().balance(&insurance.holder),
        holder_before + 8_000 * ONE
    );
    assert_eq!(insurance.pool().reserves(), reserves_before - 8_000 * ONE);
    assert_eq!(insurance.pool().total_payouts(), 8_000 * ONE);
    assert_eq!(
        insurance.token().balance(&insurance.pool_id),
        insurance.pool().reserves()
    );
}

#[test]
fn a_payout_is_capped_by_the_reserves_available() {
    let insurance = Insurance::new();
    // 1,000 units of capital plus a 500-unit premium cannot cover an 8,000
    // unit loss, so the pool pays what it holds and keeps nothing.
    insurance.deposit(&insurance.provider, 1_000 * ONE);
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.approve(claim.id);
    let reserves_before = insurance.pool().reserves();
    assert!(reserves_before < 8_000 * ONE);

    let settled = insurance.settle(claim.id);

    assert_eq!(settled.payout, reserves_before);
    assert_eq!(insurance.pool().reserves(), 0);
    assert_eq!(insurance.pool().total_payouts(), reserves_before);
}

#[test]
fn settling_a_claim_that_was_not_approved_is_rejected() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let (_, pending) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);

    let err = insurance.pool().try_settle(&pending.id);
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::NotApproved);

    insurance.cast(&insurance.assessors[0], pending.id, false);
    insurance.cast(&insurance.assessors[1], pending.id, false);
    let err = insurance.pool().try_settle(&pending.id);
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::NotApproved);
    assert_eq!(insurance.pool().total_payouts(), 0);
}

#[test]
fn settling_a_claim_twice_is_rejected() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let settled = insurance.paid_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    assert_eq!(settled.status, ClaimStatus::Settled);

    let err = insurance.pool().try_settle(&settled.id);
    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::NotApproved);
    assert_eq!(insurance.pool().total_payouts(), 8_000 * ONE);
}

#[test]
fn a_pool_with_nothing_left_refuses_to_settle() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000 * ONE);
    let (_, first) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.approve(first.id);
    insurance.settle(first.id);
    assert_eq!(insurance.pool().reserves(), 0);

    // A second policyholder owns a premium they paid into an emptied pool.
    let (_, second) = insurance.open_claim(&insurance.holder2, 10_000 * ONE, 1_000 * ONE);
    insurance.approve(second.id);
    // The premium from that policy is the only reserve left, so this one can
    // pay from it.
    assert!(insurance.pool().reserves() > 0);
    let settled = insurance.settle(second.id);
    assert!(settled.payout > 0);
}
