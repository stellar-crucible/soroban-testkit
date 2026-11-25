//! Assert: authorization, refusals and the invariants a caller can break.
//!
//! The happy paths live in `fixtures.rs`; this file is about the calls that must
//! be refused and the authorization a call must record.

mod common;

use common::{Fixture, ONE};
use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
use soroban_sdk::{IntoVal, Val};

#[test]
fn staking_authorizes_the_stake_and_the_transfer_into_the_pool() {
    let fixture = Fixture::new();
    let amount = 10 * ONE;
    let stake_args: soroban_sdk::Vec<Val> = (fixture.alice.clone(), amount).into_val(&fixture.env);
    let transfer_args: soroban_sdk::Vec<Val> =
        (fixture.alice.clone(), fixture.id.clone(), amount).into_val(&fixture.env);

    // The pool requires the staker's authorization and the token transfer it
    // makes needs the same staker again: a tree that stops at the pool call is
    // refused, which `staking_without_an_authorization_is_refused` shows from
    // the other side.
    fixture
        .client()
        .mock_auths(&[MockAuth {
            address: &fixture.alice,
            invoke: &MockAuthInvoke {
                contract: &fixture.id,
                fn_name: "stake",
                args: stake_args,
                sub_invokes: &[MockAuthInvoke {
                    contract: &fixture.stake_token,
                    fn_name: "transfer",
                    args: transfer_args,
                    sub_invokes: &[],
                }],
            },
        }])
        .stake(&fixture.alice, &amount);

    assert_eq!(fixture.client().staked(&fixture.alice), amount);
}

#[test]
#[should_panic]
fn staking_without_an_authorization_is_refused() {
    let fixture = Fixture::new();
    fixture.env.mock_auths(&[] as &[MockAuth<'_>]);

    fixture.client().stake(&fixture.alice, &(10 * ONE));
}

#[test]
#[should_panic(expected = "amount must be positive")]
fn a_zero_stake_is_refused() {
    let fixture = Fixture::new();

    fixture.client().stake(&fixture.alice, &0);
}

#[test]
#[should_panic(expected = "amount must be positive")]
fn a_negative_stake_is_refused() {
    let fixture = Fixture::new();

    fixture.client().stake(&fixture.alice, &-1);
}

#[test]
#[should_panic(expected = "already initialized")]
fn a_pool_cannot_be_initialized_twice() {
    let fixture = Fixture::new();

    fixture.client().initialize(
        &fixture.admin,
        &fixture.stake_token,
        &fixture.reward_token,
        &0,
    );
}

#[test]
#[should_panic(expected = "amount exceeds stake")]
fn unstaking_more_than_the_position_is_refused() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);

    fixture.client().unstake(&fixture.alice, &(11 * ONE));
}

#[test]
#[should_panic(expected = "no stake")]
fn claiming_without_a_position_is_refused() {
    let fixture = Fixture::new();

    fixture.client().claim(&fixture.alice);
}

#[test]
#[should_panic(expected = "pool is paused")]
fn staking_while_paused_is_refused() {
    let fixture = Fixture::new();
    fixture.client().set_paused(&true);

    fixture.client().stake(&fixture.alice, &(10 * ONE));
}

#[test]
#[should_panic(expected = "pool is not paused")]
fn an_emergency_withdrawal_needs_a_paused_pool() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);

    fixture.client().emergency_withdraw(&fixture.alice);
}

#[test]
#[should_panic(expected = "reward rate must not be negative")]
fn a_negative_reward_rate_is_refused() {
    let fixture = Fixture::new();

    fixture.client().set_reward_rate(&-1);
}

#[test]
fn a_paused_pool_still_pays_exits() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    fixture.advance_time(7 * 24 * 60 * 60);
    fixture.client().set_paused(&true);

    let returned = fixture.client().unstake(&fixture.alice, &(10 * ONE));

    assert_eq!(returned, 10 * ONE, "pausing blocks entries, never exits");
    assert_eq!(fixture.client().total_staked(), 0);
}

#[test]
fn an_emergency_withdrawal_returns_principal_and_forfeits_rewards() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    fixture.advance_time(100);
    fixture.client().set_paused(&true);

    let returned = fixture.client().emergency_withdraw(&fixture.alice);

    assert_eq!(returned, 9 * ONE, "principal minus the early-exit penalty");
    assert_eq!(
        fixture.reward_balance(&fixture.alice),
        0,
        "rewards are forfeited, not paid late"
    );
    assert_eq!(fixture.client().staked(&fixture.alice), 0);
    assert_eq!(fixture.client().earned(&fixture.alice), 0);
}

#[test]
fn a_paused_pool_keeps_accruing_for_the_stakers_who_stayed() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    fixture.client().set_paused(&true);
    fixture.advance_time(100);

    assert_eq!(fixture.client().earned(&fixture.alice), 100_000);
}

#[test]
fn an_empty_pool_does_not_pay_rewards_to_the_next_staker() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 10 * ONE);
    fixture.client().unstake(&fixture.alice, &(10 * ONE));
    fixture.advance_time(1_000);

    fixture.stake(&fixture.bob, 10 * ONE);
    fixture.advance_time(10);

    assert_eq!(
        fixture.client().earned(&fixture.bob),
        10_000,
        "the idle window belongs to nobody"
    );
}
