//! Fixtures: the staking contract driven through plain contract calls.
//!
//! Every test starts from `common::Fixture`, which deploys the pool, funds two
//! stakers and puts rewards in the contract, so what is left is the accounting
//! under test.

mod common;

use common::{Fixture, ONE};

#[test]
fn initializing_records_the_pool_configuration() {
    let fixture = Fixture::with_rate(2_500);
    let pool = fixture.client().pool();

    assert_eq!(pool.admin, fixture.admin);
    assert_eq!(pool.stake_token, fixture.stake_token);
    assert_eq!(pool.reward_token, fixture.reward_token);
    assert_eq!(pool.reward_rate, 2_500);
    assert_eq!(pool.total_staked, 0);
    assert!(!pool.paused);
}

#[test]
fn staking_moves_the_tokens_into_the_pool() {
    let fixture = Fixture::new();
    let before = fixture.stake_balance(&fixture.alice);

    let after = fixture.stake(&fixture.alice, 100 * ONE);

    assert_eq!(after, before - 100 * ONE);
    assert_eq!(
        fixture.stake_balance(&fixture.id),
        100 * ONE,
        "the pool holds what was staked"
    );
    assert_eq!(fixture.client().staked(&fixture.alice), 100 * ONE);
}

#[test]
fn total_staked_covers_every_deposit() {
    let fixture = Fixture::new();

    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.stake(&fixture.bob, 40 * ONE);

    assert_eq!(fixture.client().total_staked(), 140 * ONE);
    assert_eq!(fixture.client().staked(&fixture.alice), 100 * ONE);
    assert_eq!(fixture.client().staked(&fixture.bob), 40 * ONE);
}

#[test]
fn a_second_deposit_adds_to_the_existing_position() {
    let fixture = Fixture::new();

    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(60);
    fixture.stake(&fixture.alice, 25 * ONE);

    assert_eq!(fixture.client().staked(&fixture.alice), 125 * ONE);
    assert_eq!(
        fixture.client().total_staked(),
        125 * ONE,
        "a top-up is one position, not two"
    );
}

#[test]
fn unstaking_after_the_lock_pays_the_whole_amount() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(7 * 24 * 60 * 60);

    let returned = fixture.client().unstake(&fixture.alice, &(100 * ONE));

    assert_eq!(returned, 100 * ONE, "no penalty after the lock");
    assert_eq!(fixture.client().staked(&fixture.alice), 0);
    assert_eq!(fixture.client().total_staked(), 0);
}

#[test]
fn a_partial_unstake_leaves_the_rest_staked() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(7 * 24 * 60 * 60);

    fixture.client().unstake(&fixture.alice, &(40 * ONE));

    assert_eq!(fixture.client().staked(&fixture.alice), 60 * ONE);
    assert_eq!(fixture.client().total_staked(), 60 * ONE);
}

#[test]
fn an_early_exit_pays_a_penalty_to_the_admin() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(24 * 60 * 60);

    let returned = fixture.client().unstake(&fixture.alice, &(100 * ONE));

    assert_eq!(returned, 90 * ONE, "10% of the withdrawn amount");
    assert_eq!(fixture.stake_balance(&fixture.admin), 10 * ONE);
}

#[test]
fn claiming_pays_the_accrued_rewards_once() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);

    let first = fixture.client().claim(&fixture.alice);
    let second = fixture.client().claim(&fixture.alice);

    assert_eq!(
        first,
        1_000 * 100,
        "1,000 stroops per second for 100 seconds"
    );
    assert_eq!(second, 0, "a second claim has nothing left to pay");
    assert_eq!(fixture.reward_balance(&fixture.alice), first);
}

#[test]
fn earned_tracks_time_without_writing_to_storage() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);

    let at_zero = fixture.client().earned(&fixture.alice);
    fixture.advance_time(50);
    let later = fixture.client().earned(&fixture.alice);

    assert_eq!(at_zero, 0);
    assert_eq!(later, 1_000 * 50);
    assert_eq!(
        fixture.client().claim(&fixture.alice),
        later,
        "a view and a claim must agree"
    );
}

#[test]
fn rewards_are_split_by_stake_weight() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 300 * ONE);
    fixture.stake(&fixture.bob, 100 * ONE);
    fixture.advance_time(100);

    // 100,000 stroops over a 4:1 split.
    assert_eq!(fixture.client().earned(&fixture.alice), 75_000);
    assert_eq!(fixture.client().earned(&fixture.bob), 25_000);
}

#[test]
fn a_new_staker_does_not_share_rewards_from_before_they_staked() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);
    fixture.stake(&fixture.bob, 100 * ONE);

    assert_eq!(fixture.client().earned(&fixture.bob), 0);
    assert_eq!(fixture.client().earned(&fixture.alice), 100_000);
}

#[test]
fn changing_the_rate_settles_the_old_one_first() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);

    fixture.client().set_reward_rate(&2_000);
    fixture.advance_time(100);

    assert_eq!(
        fixture.client().earned(&fixture.alice),
        100_000 + 200_000,
        "100 seconds at 1,000 then 100 seconds at 2,000"
    );
}

#[test]
fn rewards_accrue_across_ledger_sequences_as_well_as_clock_time() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);

    // advance_time moves both the timestamp and the sequence number, so a
    // contract that used either clock would see the same elapsed window.
    fixture.advance_time(10);
    assert_eq!(fixture.client().earned(&fixture.alice), 10_000);
}

#[test]
fn a_stake_after_a_claim_only_earns_on_the_new_amount() {
    let fixture = Fixture::new();
    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);
    fixture.client().claim(&fixture.alice);

    fixture.stake(&fixture.alice, 100 * ONE);
    fixture.advance_time(100);

    assert_eq!(fixture.client().earned(&fixture.alice), 100_000);
    assert_eq!(fixture.client().staked(&fixture.alice), 200 * ONE);
}
