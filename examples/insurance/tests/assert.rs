//! What the pool publishes and authorizes, checked with
//! `soroban-testkit-assert`: events across the pool and the capital token,
//! event payloads, signers, and that a settlement only moves tokens.

mod common;

use common::*;
use soroban_sdk::Address;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{
    assert_auth_matches, assert_balance_conserved, assert_event_count_range,
    assert_event_topic_count,
};
use soroban_testkit_example_insurance::{ClaimStatus, InsuranceError};

#[test]
fn a_deposit_publishes_its_event_and_a_token_transfer() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;

    insurance.deposit(&insurance.provider, 1_000 * ONE);

    assert_event_topic_count!(env, from = insurance.pool_id, "deposit", 1);
    assert_event_topic_count!(env, from = insurance.token, "transfer", 1);
    assert_event_count_range!(env, 2);
    EventMatcher::new(env)
        .from_contract(&insurance.pool_id)
        .with_topic("deposit")
        .assert_data_matches(|data| {
            data.deserialize::<(i128, i128)>() == Some((1_000 * ONE, 1_000 * ONE))
        });
}

#[test]
fn a_policy_reports_its_id_coverage_and_premium() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;

    insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);

    assert_event_topic_count!(env, from = insurance.pool_id, "policy", 1);
    assert_event_topic_count!(env, from = insurance.token, "transfer", 1);
    EventMatcher::new(env)
        .from_contract(&insurance.pool_id)
        .with_topic("policy")
        .assert_data_matches(|data| {
            data.deserialize::<(u64, i128, i128)>() == Some((1, 10_000 * ONE, 500 * ONE))
        });
}

#[test]
fn a_filed_claim_names_the_policy_and_the_loss() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);

    let claim = insurance.file(&insurance.holder, policy.id, 8_000 * ONE);

    assert_event_topic_count!(env, from = insurance.pool_id, "claim", 1);
    assert_event_count_range!(env, 1);
    EventMatcher::new(env)
        .from_contract(&insurance.pool_id)
        .with_topic("claim")
        .assert_data_matches(move |data| {
            data.deserialize::<(u64, u64, i128)>() == Some((claim.id, policy.id, 8_000 * ONE))
        });
}

#[test]
fn a_vote_records_the_assessor_and_the_decision() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);

    insurance.cast(&insurance.assessors[0], claim.id, true);

    assert_event_topic_count!(env, from = insurance.pool_id, "vote", 1);
    // The first vote leaves the claim pending; the second resolves it.
    EventMatcher::new(env)
        .from_contract(&insurance.pool_id)
        .with_topic("vote")
        .assert_data_matches(move |data| {
            data.deserialize::<(u64, bool, ClaimStatus)>()
                == Some((claim.id, true, ClaimStatus::Pending))
        });
}

#[test]
fn a_settlement_publishes_one_payout_event_and_one_transfer() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.approve(claim.id);

    let settled = insurance.settle(claim.id);

    assert_event_topic_count!(env, from = insurance.pool_id, "settle", 1);
    assert_event_topic_count!(env, from = insurance.token, "transfer", 1);
    assert_event_count_range!(env, 2);
    EventMatcher::new(env)
        .from_contract(&insurance.pool_id)
        .with_topic("settle")
        .assert_data_matches(move |data| {
            data.deserialize::<(u64, i128)>() == Some((settled.id, settled.payout))
        });
}

#[test]
fn a_rejected_claim_pays_nothing() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.cast(&insurance.assessors[0], claim.id, true);
    let resolved = insurance.cast(&insurance.assessors[1], claim.id, false);
    assert_eq!(resolved.status, ClaimStatus::Rejected);

    let err = insurance.pool().try_settle(&claim.id);

    assert_eq!(err.unwrap_err().unwrap(), InsuranceError::NotApproved);
    EventMatcher::new(env)
        .with_topic("settle")
        .assert_not_emitted();
    assert_eq!(insurance.pool().total_payouts(), 0);
    assert_eq!(
        insurance.token().balance(&insurance.holder),
        FUNDING - 500 * ONE
    );
}

#[test]
fn every_mutation_requires_its_actor() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;

    insurance.deposit(&insurance.provider, 1_000 * ONE);
    assert_auth_matches!(env, {
        invoker: insurance.provider,
        function: "deposit",
        args: [insurance.provider.clone(), 1_000 * ONE],
    });

    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    assert_auth_matches!(env, {
        invoker: insurance.holder,
        function: "buy_policy",
        args: [insurance.holder.clone(), 10_000 * ONE, YEAR],
    });

    let claim = insurance.file(&insurance.holder, policy.id, 8_000 * ONE);
    assert_auth_matches!(env, {
        invoker: insurance.holder,
        function: "file_claim",
        args: [insurance.holder.clone(), policy.id, 8_000 * ONE],
    });

    insurance.cast(&insurance.assessors[0], claim.id, true);
    assert_auth_matches!(env, {
        invoker: insurance.assessors[0],
        function: "vote",
        args: [insurance.assessors[0].clone(), claim.id, true],
    });

    insurance.pool().withdraw(&insurance.provider, &(500 * ONE));
    assert_auth_matches!(env, {
        invoker: insurance.provider,
        function: "withdraw",
        args: [insurance.provider.clone(), 500 * ONE],
    });
}

#[test]
fn a_settlement_conserves_the_capital_token() {
    let insurance = Insurance::new();
    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    let (_, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
    insurance.approve(claim.id);

    let settled = assert_balance_conserved!(
        accounts: [
            (&insurance.pool_id, "pool"),
            (&insurance.holder, "holder"),
        ],
        balance: |addr: &Address| insurance.token().balance(addr),
        call: {
            insurance.settle(claim.id)
        }
    );

    assert_eq!(settled.payout, 8_000 * ONE);
    assert_eq!(insurance.pool().reserves(), 1_000_500 * ONE - 8_000 * ONE);
}

#[test]
fn a_deposit_moves_capital_without_minting_any() {
    let insurance = Insurance::new();

    assert_balance_conserved!(
        accounts: [
            (&insurance.provider, "provider"),
            (&insurance.pool_id, "pool"),
        ],
        balance: |addr: &Address| insurance.token().balance(addr),
        mint: 0,
        burn: 0,
        call: {
            insurance.deposit(&insurance.provider, 1_000 * ONE);
        }
    );

    assert_eq!(
        insurance.token().balance(&insurance.provider),
        FUNDING - 1_000 * ONE
    );
    assert_eq!(insurance.token().balance(&insurance.pool_id), 1_000 * ONE);
}

#[test]
fn an_event_log_counts_a_whole_pool_lifecycle() {
    let insurance = Insurance::new();
    let env = &insurance.ctx.env;
    let mut log = EventLog::new(env);

    insurance.deposit(&insurance.provider, 1_000_000 * ONE);
    log.collect();
    let policy = insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);
    log.collect();
    let claim = insurance.file(&insurance.holder, policy.id, 8_000 * ONE);
    log.collect();
    insurance.cast(&insurance.assessors[0], claim.id, true);
    log.collect();
    insurance.cast(&insurance.assessors[1], claim.id, true);
    log.collect();
    insurance.settle(claim.id);
    log.collect();

    assert_event_topic_count!(log, from = insurance.pool_id, "deposit", 1);
    assert_event_topic_count!(log, from = insurance.pool_id, "policy", 1);
    assert_event_topic_count!(log, from = insurance.pool_id, "claim", 1);
    assert_event_topic_count!(log, from = insurance.pool_id, "vote", 2);
    assert_event_topic_count!(log, from = insurance.pool_id, "settle", 1);
    assert_event_topic_count!(log, from = insurance.token, "transfer", 3);
    assert_event_count_range!(log, 9);
}
