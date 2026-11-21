//! Property tests: rewards conserve, principal comes back, nothing goes negative.
//!
//! The properties are about the accounting, not the harness: every case builds a
//! fresh pool with a rate of 1,000 stroops per second and moves generated
//! amounts in and out of it.

mod common;

use common::{Fixture, ONE};
use proptest::prelude::*;

const LOCK: u64 = 7 * 24 * 60 * 60;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn total_staked_equals_the_sum_of_every_position(
        alice in 1i128..=1_000 * ONE,
        bob in 1i128..=1_000 * ONE,
    ) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &alice);
        fixture.client().stake(&fixture.bob, &bob);

        prop_assert_eq!(
            fixture.client().total_staked(),
            fixture.client().staked(&fixture.alice) + fixture.client().staked(&fixture.bob)
        );
    }

    #[test]
    fn principal_survives_a_full_round_trip_after_the_lock(amount in 1i128..=1_000 * ONE) {
        let fixture = Fixture::new();
        let before = fixture.stake_balance(&fixture.alice);

        fixture.client().stake(&fixture.alice, &amount);
        fixture.advance_time(LOCK);

        let returned = fixture.client().unstake(&fixture.alice, &amount);

        prop_assert_eq!(returned, amount, "no penalty after the lock");
        prop_assert_eq!(fixture.stake_balance(&fixture.alice), before);
        prop_assert_eq!(fixture.client().total_staked(), 0);
    }

    #[test]
    fn an_early_exit_loses_exactly_the_penalty(amount in 1i128..=1_000 * ONE) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &amount);
        fixture.advance_time(60);

        let returned = fixture.client().unstake(&fixture.alice, &amount);
        let penalty = amount * 1_000 / 10_000;

        prop_assert_eq!(returned, amount - penalty, "fee split lost a stroop");
        prop_assert_eq!(returned + penalty, amount);
        prop_assert!(returned >= 0);
        prop_assert!(penalty >= 0);
    }

    #[test]
    fn rewards_never_exceed_the_stream(amount in 1i128..=1_000 * ONE, seconds in 1u64..=1_000) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &amount);
        fixture.advance_time(seconds);

        let earned = fixture.client().earned(&fixture.alice);

        prop_assert!(earned >= 0);
        prop_assert!(
            earned <= 1_000 * seconds as i128,
            "earned {earned} exceeded the {seconds}s stream"
        );
    }

    #[test]
    fn two_stakers_share_the_stream_without_creating_value(
        alice in 1i128..=1_000 * ONE,
        bob in 1i128..=1_000 * ONE,
        seconds in 1u64..=1_000,
    ) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &alice);
        fixture.client().stake(&fixture.bob, &bob);
        fixture.advance_time(seconds);

        let total = fixture.client().earned(&fixture.alice) + fixture.client().earned(&fixture.bob);
        let stream = 1_000 * seconds as i128;

        prop_assert!(total <= stream, "two claims of {total} exceed the {stream} stream");
        // Integer division rounds down twice, so at most one stroop per staker.
        prop_assert!(stream - total <= 2, "the split lost {} stroops", stream - total);
    }

    #[test]
    fn a_later_staker_never_receives_earlier_rewards(
        first in 1i128..=1_000 * ONE,
        second in 1i128..=1_000 * ONE,
        seconds in 1u64..=1_000,
    ) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &first);
        fixture.advance_time(seconds);
        let before = fixture.client().earned(&fixture.alice);

        fixture.client().stake(&fixture.bob, &second);

        prop_assert_eq!(fixture.client().earned(&fixture.bob), 0);
        prop_assert_eq!(fixture.client().earned(&fixture.alice), before);
    }

    #[test]
    fn claiming_twice_pays_the_same_as_claiming_once(
        amount in 1i128..=1_000 * ONE,
        seconds in 1u64..=1_000,
    ) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &amount);
        fixture.advance_time(seconds);

        let once = fixture.client().claim(&fixture.alice);
        let twice = fixture.client().claim(&fixture.alice);

        prop_assert!(once >= 0);
        prop_assert_eq!(twice, 0, "the second claim found nothing left");
        prop_assert_eq!(fixture.reward_balance(&fixture.alice), once);
    }

    #[test]
    fn the_position_never_holds_more_than_the_pool(
        alice in 1i128..=1_000 * ONE,
        withdraw in 1i128..=1_000 * ONE,
    ) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &alice);
        fixture.advance_time(LOCK);
        let withdraw = withdraw.min(alice);

        let returned = fixture.client().unstake(&fixture.alice, &withdraw);

        prop_assert_eq!(returned, withdraw);
        prop_assert_eq!(
            fixture.client().staked(&fixture.alice) + withdraw,
            alice
        );
        prop_assert!(fixture.client().total_staked() >= 0);
        prop_assert_eq!(fixture.client().total_staked(), alice - withdraw);
    }
}
