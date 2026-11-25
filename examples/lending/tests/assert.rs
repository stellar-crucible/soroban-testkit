//! Protocol observability checked with `soroban-testkit-assert`: which
//! contract published which event, what the payload carried, who authorized
//! each call, and whether collateral and debt balances stayed conserved.

mod common;

use common::*;
use soroban_sdk::Address;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{
    assert_auth_matches, assert_balance_conserved, assert_event_count_range,
    assert_event_topic_count,
};
use soroban_testkit_example_lending::PoolError;

#[test]
fn deposit_publishes_a_vault_event_and_a_collateral_transfer() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    let amount = 1_000 * ONE;

    lending.vault().deposit(&lending.owner, &amount);

    assert_event_topic_count!(env, from = lending.vault_id, "deposit", 1);
    assert_event_topic_count!(env, from = lending.collateral, "transfer", 1);
    assert_event_count_range!(env, 2);
    EventMatcher::new(env)
        .from_contract(&lending.vault_id)
        .with_topic("deposit")
        .assert_data_matches(|data| data.deserialize::<i128>() == Some(amount));
}

#[test]
fn borrow_publishes_a_pool_event_and_a_debt_mint() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.deposit(&lending.owner, 10_000 * ONE);

    lending.borrow(&lending.owner, 6_000 * ONE);

    assert_event_topic_count!(env, from = lending.pool_id, "borrow", 1);
    assert_event_topic_count!(env, from = lending.debt_id, "mint", 1);
    assert_event_count_range!(env, 2);
    EventMatcher::new(env)
        .from_contract(&lending.pool_id)
        .with_topic("borrow")
        .assert_data_matches(|data| data.deserialize::<i128>() == Some(6_000 * ONE));
}

#[test]
fn a_rejected_borrow_publishes_no_pool_or_mint_event() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.deposit(&lending.owner, 10_000 * ONE);

    let err = lending
        .pool()
        .try_borrow(&lending.owner, &(8_000 * ONE + 1));

    assert_eq!(err.unwrap_err().unwrap(), PoolError::AboveMaxLtv);
    EventMatcher::new(env)
        .with_topic("borrow")
        .assert_not_emitted();
    EventMatcher::new(env)
        .with_topic("mint")
        .assert_not_emitted();
    assert_eq!(lending.debt().total_supply(), 0);
}

#[test]
fn interest_publishes_a_named_event_with_the_accrued_amount() {
    let mut lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.advance_time(YEAR);

    lending.pool().accrue(&lending.owner);

    let env = &lending.ctx.env;
    assert_event_topic_count!(env, from = lending.pool_id, "interest", 1);
    EventMatcher::new(env)
        .from_contract(&lending.pool_id)
        .with_topic("interest")
        .assert_data_matches(|data| data.deserialize::<i128>() == Some(300 * ONE));
}

#[test]
fn liquidation_events_span_pool_debt_token_and_vault() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.arm_liquidator(6_000 * ONE);
    lending.set_price(6_500_000);

    let result = lending
        .pool()
        .liquidate(&lending.liquidator, &lending.owner);

    assert_event_topic_count!(env, from = lending.pool_id, "liquidate", 1);
    assert_event_topic_count!(env, from = lending.debt_id, "burn", 1);
    assert_event_topic_count!(env, from = lending.vault_id, "seize", 1);
    assert_event_topic_count!(env, from = lending.collateral, "transfer", 1);
    assert_event_count_range!(env, 4..=5);
    EventMatcher::new(env)
        .from_contract(&lending.pool_id)
        .with_topic("liquidate")
        .assert_data_matches(|data| {
            data.deserialize::<(i128, i128)>()
                == Some((result.debt_repaid, result.collateral_seized))
        });
}

#[test]
fn an_event_log_counts_a_whole_lifecycle() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    let mut log = EventLog::new(env);

    lending.vault().deposit(&lending.owner, &(10_000 * ONE));
    log.collect();
    lending.pool().borrow(&lending.owner, &(4_000 * ONE));
    log.collect();
    lending.pool().borrow(&lending.owner, &(2_000 * ONE));
    log.collect();
    lending.pool().repay(&lending.owner, &(6_000 * ONE));
    log.collect();
    lending.vault().withdraw(&lending.owner, &(10_000 * ONE));
    log.collect();

    assert_event_topic_count!(log, from = lending.pool_id, "borrow", 2);
    assert_event_topic_count!(log, from = lending.pool_id, "repay", 1);
    assert_event_topic_count!(log, from = lending.debt_id, "mint", 2);
    assert_event_topic_count!(log, from = lending.debt_id, "burn", 1);
    assert_event_topic_count!(log, from = lending.collateral, "transfer", 2);
    assert_event_topic_count!(log, from = lending.vault_id, "liquidate", 0);
    assert_event_count_range!(log, 10);
}

#[test]
fn deposit_requires_the_owner_to_authorize_the_exact_amount() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    let amount = 1_000 * ONE;

    lending.vault().deposit(&lending.owner, &amount);

    assert_auth_matches!(env, {
        invoker: lending.owner,
        function: "deposit",
        args: [lending.owner, amount],
    });
}

#[test]
fn borrow_repay_and_liquidate_require_their_respective_callers() {
    let lending = Lending::new();
    let env = &lending.ctx.env;
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);

    lending.pool().repay(&lending.owner, &(1_000 * ONE));
    assert_auth_matches!(env, {
        invoker: lending.owner,
        function: "repay",
        args: [_, 1_000 * ONE],
    });

    lending.arm_liquidator(6_000 * ONE);
    lending.set_price(5_000_000);
    lending
        .pool()
        .liquidate(&lending.liquidator, &lending.owner);
    assert_auth_matches!(env, {
        invoker: lending.liquidator,
        function: "liquidate",
        args: [lending.liquidator, lending.owner],
    });
}

#[test]
fn liquidation_only_moves_collateral_between_vault_and_liquidator() {
    let lending = Lending::new();
    lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE);
    lending.arm_liquidator(6_000 * ONE);
    lending.set_price(6_500_000);
    let collateral = lending.collateral_token();

    assert_balance_conserved!(
        accounts: [
            (&lending.owner, "owner"),
            (&lending.liquidator, "liquidator"),
            (&lending.vault_id, "vault"),
        ],
        balance: |addr: &Address| collateral.balance(addr),
        call: {
            lending.pool().liquidate(&lending.liquidator, &lending.owner);
        }
    );
}

#[test]
fn a_borrow_mints_exactly_the_debt_token_supply() {
    let lending = Lending::new();
    lending.deposit(&lending.owner, 10_000 * ONE);
    let debt = lending.debt();

    assert_balance_conserved!(
        accounts: [(&lending.owner, "owner"), (&lending.liquidator, "liquidator")],
        balance: |addr: &Address| debt.balance(addr),
        mint: 500 * ONE,
        burn: 0,
        call: {
            lending.pool().borrow(&lending.owner, &(500 * ONE));
        }
    );
    assert_eq!(debt.total_supply(), 500 * ONE);
}
