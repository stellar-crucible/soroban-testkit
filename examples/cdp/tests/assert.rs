//! What the protocol publishes and authorizes, checked with
//! `soroban-testkit-assert`: event counts and topic frequencies across contracts
//! (including the auxiliary `fee` event that makes exact counts brittle),
//! event payloads, authorization requirements, and token conservation.

mod common;

use common::*;
use soroban_sdk::Address;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{
    assert_auth_matches, assert_balance_conserved, assert_event_count_range,
    assert_event_topic_count,
};
use soroban_testkit_example_cdp::CdpError;

#[test]
fn deposit_publishes_a_deposit_event_and_a_collateral_transfer() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;

    cdp.engine().deposit(&cdp.owner, &(1_000 * ONE));

    assert_event_topic_count!(env, from = cdp.engine_id, "deposit", 1);
    assert_event_topic_count!(env, from = cdp.collateral, "transfer", 1);
    assert_event_count_range!(env, 2);
    EventMatcher::new(env)
        .with_topic("deposit")
        .assert_data_matches(|data| data.deserialize::<i128>() == Some(1_000 * ONE));
}

#[test]
fn borrow_spans_two_contracts_a_borrow_and_a_mint() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    cdp.engine().borrow(&cdp.owner, &(600 * ONE));

    assert_event_topic_count!(env, from = cdp.engine_id, "borrow", 1);
    assert_event_topic_count!(env, from = cdp.synth_id, "mint", 1);
    assert_event_topic_count!(env, "fee", 0);
    assert_event_count_range!(env, from = cdp.synth_id, 1);
}

#[test]
fn a_range_absorbs_the_auxiliary_fee_event_an_exact_count_would_trip_on() {
    let mut cdp = Cdp::new();
    let env = cdp.ctx.env.clone();
    cdp.open(&cdp.owner, 10_000 * ONE, 300 * ONE);

    // No time has passed: borrow + mint.
    cdp.engine().borrow(&cdp.owner, &(100 * ONE));
    assert_event_count_range!(env, 2..=3);
    assert_event_topic_count!(env, "fee", 0);

    // A year later the same call also folds in the stability fee: borrow +
    // mint + fee. The range still holds where `assert_count(2)` would not.
    cdp.advance_time(YEAR);
    cdp.engine().borrow(&cdp.owner, &(100 * ONE));
    assert_event_count_range!(env, 2..=3);
    assert_event_topic_count!(env, "fee", 1);
    EventMatcher::new(&env)
        .with_topic("fee")
        .assert_data_matches(|data| data.deserialize::<i128>() == Some(8 * ONE));
}

#[test]
fn liquidation_events_carry_the_repaid_debt_and_seized_collateral() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(600 * ONE);
    cdp.crash_price(20);

    cdp.engine().liquidate(&cdp.liquidator, &cdp.owner);

    // liquidate (engine) + burn (sUSD) + transfer (XLM), and a fee event when
    // time has passed since the last touch.
    assert_event_count_range!(env, 3..=4);
    assert_event_topic_count!(env, from = cdp.engine_id, "liquidate", 1);
    assert_event_topic_count!(env, from = cdp.synth_id, "burn", 1);
    assert_event_topic_count!(env, from = cdp.collateral, "transfer", 1);
    EventMatcher::new(env)
        .from_contract(&cdp.engine_id)
        .with_topic("liquidate")
        .assert_data_matches(|data| {
            data.deserialize::<(i128, i128)>() == Some((600 * ONE, 8_250 * ONE))
        });
}

#[test]
fn a_rejected_borrow_mints_nothing() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    let err = cdp.engine().try_borrow(&cdp.owner, &(700 * ONE));

    assert_eq!(err.unwrap_err().unwrap(), CdpError::BelowMinCollateralRatio);
    EventMatcher::new(env)
        .with_topic("mint")
        .assert_not_emitted();
    assert_eq!(cdp.synth().total_supply(), 0);
}

#[test]
fn an_event_log_counts_a_whole_position_lifecycle() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    let mut log = EventLog::new(env);

    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));
    log.collect();
    cdp.engine().borrow(&cdp.owner, &(500 * ONE));
    log.collect();
    cdp.engine().borrow(&cdp.owner, &(100 * ONE));
    log.collect();
    cdp.engine().repay(&cdp.owner, &(600 * ONE));
    log.collect();
    cdp.engine().withdraw(&cdp.owner, &(10_000 * ONE));
    log.collect();

    assert_event_topic_count!(log, "borrow", 2);
    assert_event_topic_count!(log, from = cdp.synth_id, "mint", 2);
    assert_event_topic_count!(log, from = cdp.synth_id, "burn", 1);
    assert_event_topic_count!(log, from = cdp.collateral, "transfer", 2);
    assert_event_topic_count!(log, "liquidate", 0);
    assert_event_count_range!(log, 9..=12);
}

#[test]
fn deposit_requires_the_owner_to_authorize_the_exact_amount() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    let amount = 1_000 * ONE;

    cdp.engine().deposit(&cdp.owner, &amount);

    assert_auth_matches!(env, {
        invoker: cdp.owner,
        function: "deposit",
        args: [cdp.owner, amount],
    });
}

#[test]
fn borrow_and_repay_require_the_owner_and_liquidate_requires_the_liquidator() {
    let cdp = Cdp::new();
    let env = &cdp.ctx.env;
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));

    cdp.engine().borrow(&cdp.owner, &(600 * ONE));
    assert_auth_matches!(env, { invoker: cdp.owner, function: "borrow" });

    cdp.engine().repay(&cdp.owner, &(100 * ONE));
    assert_auth_matches!(env, { invoker: cdp.owner, function: "repay", args: [_, 100 * ONE] });

    cdp.arm_liquidator(500 * ONE);
    cdp.crash_price(30);
    cdp.engine().liquidate(&cdp.liquidator, &cdp.owner);
    assert_auth_matches!(env, {
        invoker: cdp.liquidator,
        function: "liquidate",
        args: [cdp.liquidator, cdp.owner],
    });
}

#[test]
fn liquidation_conserves_collateral_between_engine_and_liquidator() {
    let cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.arm_liquidator(600 * ONE);
    cdp.crash_price(25);
    let xlm = cdp.xlm();

    let result = assert_balance_conserved!(
        accounts: [
            (&cdp.owner, "owner"),
            (&cdp.liquidator, "liquidator"),
            (&cdp.engine_id, "engine"),
        ],
        balance: |addr: &Address| xlm.balance(addr),
        call: {
            cdp.engine().liquidate(&cdp.liquidator, &cdp.owner)
        }
    );
    assert!(result.collateral_seized > 0);
}

#[test]
fn borrowing_mints_exactly_the_new_supply_and_nothing_else_moves() {
    let cdp = Cdp::new();
    cdp.engine().deposit(&cdp.owner, &(10_000 * ONE));
    let synth = cdp.synth();

    assert_balance_conserved!(
        accounts: [(&cdp.owner, "owner"), (&cdp.liquidator, "liquidator")],
        balance: |addr: &Address| synth.balance(addr),
        mint: 250 * ONE,
        burn: 0,
        call: {
            cdp.engine().borrow(&cdp.owner, &(250 * ONE));
        }
    );
}

#[test]
fn total_debt_is_supply_plus_accrued_fees() {
    let mut cdp = Cdp::new();
    cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE);
    cdp.open(&cdp.other, 20_000 * ONE, 400 * ONE);

    cdp.advance_time(YEAR);
    cdp.engine().accrue(&cdp.owner);
    cdp.engine().accrue(&cdp.other);

    // 2% of 600 + 2% of 400 = 20 sUSD of fees owed but never minted.
    let fees = cdp.engine().total_debt() - cdp.synth().total_supply();
    assert_eq!(fees, 20 * ONE);
}
