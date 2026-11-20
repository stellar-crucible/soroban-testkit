//! Strategies over time: durations, timepoints and ledger intervals.
//!
//! `soroban-sdk` 28 represents both [`Duration`] and [`Timepoint`] as 64-bit
//! unsigned second counts. They are environment-backed wrappers, not plain
//! integers: a value is built with `Duration::from_seconds(&env, ..)` or
//! `Timepoint::from_unix(&env, ..)` and read back with `to_seconds()` or
//! `to_unix()`. The strategies therefore take an [`Env`], the same way
//! [`fuzzed_invocation_args`](crate::strategies::fuzzed_invocation_args) does,
//! so every generated value belongs to the environment the test uses.
//!
//! Every strategy shrinks towards the small end of its range: a duration
//! towards `min_secs`, a timepoint towards `min_timestamp`, and a ledger
//! interval towards a zero-length interval at the lowest start. A vesting or
//! timelock bug is reported at the smallest span that still reproduces it.

use proptest::prelude::*;
use soroban_sdk::{Duration, Env, Timepoint};

/// The highest ledger sequence [`ledger_sequence_interval`] generates, matching
/// [`ledger_sequence`](crate::strategies::ledger_sequence).
pub const MAX_LEDGER_SEQUENCE: u32 = 10_000_000;

/// Generates a [`Duration`] whose seconds lie in `min_secs..=max_secs`.
///
/// The value is a host `Duration`, so it can be passed straight to a contract
/// entry point and compares as the contract sees it. Shrinking moves the
/// duration towards `min_secs`, so a failing vesting or timelock property is
/// reported at the shortest duration that still fails.
///
/// # Panics
///
/// Panics when `min_secs > max_secs`, since no duration satisfies both. The
/// full `u64` range is accepted, including `u64::MAX`.
///
/// ```
/// use proptest::strategy::{Strategy as _, ValueTree};
/// use proptest::test_runner::TestRunner;
/// use soroban_sdk::Env;
/// use soroban_testkit_generators::temporal::soroban_duration;
///
/// let env = Env::default();
/// let mut runner = TestRunner::deterministic();
/// let duration = soroban_duration(&env, 1, 86_400)
///     .new_tree(&mut runner)
///     .unwrap()
///     .current();
/// assert!(duration.to_seconds() >= 1);
/// ```
pub fn soroban_duration(
    env: &Env,
    min_secs: u64,
    max_secs: u64,
) -> impl Strategy<Value = Duration> {
    assert!(
        min_secs <= max_secs,
        "soroban_duration: min_secs ({min_secs}) is above max_secs ({max_secs}), so no duration exists"
    );
    let env = env.clone();
    (min_secs..=max_secs).prop_map(move |seconds| Duration::from_seconds(&env, seconds))
}

/// Generates a [`Timepoint`] whose unix timestamp lies in
/// `min_timestamp..=max_timestamp`.
///
/// Shrinking moves the timepoint towards `min_timestamp`, so an off-by-one in
/// an unlock check minimises to the earliest instant that still fails.
///
/// # Panics
///
/// Panics when `min_timestamp > max_timestamp`, since no timepoint satisfies
/// both.
///
/// ```
/// use proptest::strategy::{Strategy as _, ValueTree};
/// use proptest::test_runner::TestRunner;
/// use soroban_sdk::Env;
/// use soroban_testkit_generators::temporal::soroban_timepoint;
///
/// let env = Env::default();
/// let mut runner = TestRunner::deterministic();
/// let unlock = soroban_timepoint(&env, 1, 2_000_000_000)
///     .new_tree(&mut runner)
///     .unwrap()
///     .current();
/// assert!(unlock.to_unix() >= 1);
/// ```
pub fn soroban_timepoint(
    env: &Env,
    min_timestamp: u64,
    max_timestamp: u64,
) -> impl Strategy<Value = Timepoint> {
    assert!(
        min_timestamp <= max_timestamp,
        "soroban_timepoint: min_timestamp ({min_timestamp}) is above max_timestamp ({max_timestamp}), so no timepoint exists"
    );
    let env = env.clone();
    (min_timestamp..=max_timestamp).prop_map(move |seconds| Timepoint::from_unix(&env, seconds))
}

/// Generates a ledger-sequence interval `(start, end)` with `start <= end`.
///
/// Both ends lie in `1..=MAX_LEDGER_SEQUENCE`, the range
/// [`ledger_sequence`](crate::strategies::ledger_sequence) draws a single ledger
/// from. Shrinking moves both ends towards `1`, so a failing interval becomes
/// `(1, 1)` — a single-ledger span at the earliest realistic ledger — rather
/// than an arbitrary pair.
pub fn ledger_sequence_interval() -> impl Strategy<Value = (u32, u32)> {
    (1u32..=MAX_LEDGER_SEQUENCE, 1u32..=MAX_LEDGER_SEQUENCE)
        .prop_map(|(left, right)| (left.min(right), left.max(right)))
}

#[cfg(test)]
mod tests {
    use super::{
        ledger_sequence_interval, soroban_duration, soroban_timepoint, MAX_LEDGER_SEQUENCE,
    };
    use proptest::prelude::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::Env;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        #[test]
        fn durations_stay_inside_the_requested_range(
            duration in soroban_duration(&Env::default(), 60, 86_400),
        ) {
            prop_assert!((60..=86_400).contains(&duration.to_seconds()));
        }

        #[test]
        fn timepoints_stay_inside_the_requested_range(
            timepoint in soroban_timepoint(&Env::default(), 1_600_000_000, 2_000_000_000),
        ) {
            prop_assert!((1_600_000_000..=2_000_000_000).contains(&timepoint.to_unix()));
        }

        #[test]
        fn intervals_are_ordered_and_bounded(interval in ledger_sequence_interval()) {
            let (start, end) = interval;
            prop_assert!(start <= end);
            prop_assert!((1..=MAX_LEDGER_SEQUENCE).contains(&start));
            prop_assert!((1..=MAX_LEDGER_SEQUENCE).contains(&end));
        }
    }

    /// Drive proptest's shrinker by hand: keep simplifying while `fails` holds.
    fn minimise<T: Strategy>(strategy: T, fails: impl Fn(&T::Value) -> bool) -> T::Value {
        let mut runner = TestRunner::deterministic();
        let mut tree = loop {
            let tree = strategy.new_tree(&mut runner).unwrap();
            if fails(&tree.current()) {
                break tree;
            }
        };
        let mut smallest = tree.current();
        loop {
            if fails(&tree.current()) {
                smallest = tree.current();
                if !tree.simplify() {
                    break;
                }
            } else if !tree.complicate() {
                break;
            }
        }
        smallest
    }

    #[test]
    fn a_failing_duration_shrinks_to_min_secs() {
        let env = Env::default();
        // A vesting contract that wrongly treats anything below 30 days as
        // fully vested.
        let smallest = minimise(soroban_duration(&env, 600, 86_400), |duration| {
            duration.to_seconds() < 2_592_000
        });
        assert_eq!(smallest.to_seconds(), 600);
    }

    #[test]
    fn a_failing_timepoint_shrinks_to_min_timestamp() {
        let env = Env::default();
        let smallest = minimise(
            soroban_timepoint(&env, 1_600_000_000, 2_000_000_000),
            |timepoint| timepoint.to_unix() < 1_700_000_000,
        );
        assert_eq!(smallest.to_unix(), 1_600_000_000);
    }

    #[test]
    fn a_failing_interval_shrinks_to_a_single_ledger() {
        let smallest = minimise(ledger_sequence_interval(), |_| true);
        assert_eq!(smallest, (1, 1));
    }

    #[test]
    fn a_failing_interval_shrinks_towards_the_lowest_start() {
        // Only a non-empty span fails: shrinking reduces the start to 1 and the
        // span to the smallest value that still reproduces the failure.
        let smallest = minimise(ledger_sequence_interval(), |(start, end)| start < end);
        assert_eq!(smallest, (1, 2));
    }

    #[test]
    fn a_single_second_range_generates_exactly_that_second() {
        let env = Env::default();
        let mut runner = TestRunner::deterministic();
        for _ in 0..16 {
            let duration = soroban_duration(&env, 7, 7)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            assert_eq!(duration.to_seconds(), 7);
        }
    }

    #[test]
    fn a_zero_duration_is_a_valid_generated_value() {
        let env = Env::default();
        let mut runner = TestRunner::deterministic();
        let duration = soroban_duration(&env, 0, 0)
            .new_tree(&mut runner)
            .unwrap()
            .current();
        assert_eq!(duration.to_seconds(), 0);
    }

    #[test]
    fn the_top_of_the_u64_range_does_not_overflow() {
        let env = Env::default();
        let mut runner = TestRunner::deterministic();
        for _ in 0..64 {
            let duration = soroban_duration(&env, u64::MAX - 10, u64::MAX)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            assert!(duration.to_seconds() >= u64::MAX - 10);

            let timepoint = soroban_timepoint(&env, u64::MAX - 10, u64::MAX)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            assert!(timepoint.to_unix() >= u64::MAX - 10);
        }
    }

    #[test]
    #[should_panic(expected = "min_secs (10) is above max_secs (5)")]
    fn an_empty_duration_range_is_refused() {
        let _ = soroban_duration(&Env::default(), 10, 5);
    }

    #[test]
    #[should_panic(expected = "min_timestamp (10) is above max_timestamp (5)")]
    fn an_empty_timepoint_range_is_refused() {
        let _ = soroban_timepoint(&Env::default(), 10, 5);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// Linear vesting: `amount * elapsed / duration`, clamped at both ends.
        /// The generated duration and timepoint are host values, and the
        /// multiplication is checked so an overflow would fail the property
        /// rather than wrap.
        #[test]
        fn vesting_never_underflows_or_overpays(
            (duration, start) in {
                let env = Env::default();
                (
                    soroban_duration(&env, 1, 10_000_000),
                    soroban_timepoint(&env, 1_600_000_000, 2_000_000_000),
                )
            },
            elapsed in 0u64..=10_000_000u64,
            amount in 0u128..=1_000_000_000_000_000u128,
        ) {
            let total = duration.to_seconds();
            let elapsed = elapsed.min(total);
            let vested = amount
                .checked_mul(u128::from(elapsed))
                .expect("the bounded amount cannot overflow u128")
                / u128::from(total);

            prop_assert!(vested <= amount);
            prop_assert_eq!(
                vested,
                amount.checked_mul(u128::from(elapsed)).unwrap() / u128::from(total)
            );
            if elapsed == 0 {
                prop_assert_eq!(vested, 0);
            }
            if elapsed == total {
                prop_assert_eq!(vested, amount);
            }
            prop_assert!(start.to_unix() >= 1_600_000_000);
        }

        /// A timelock unlocks at `start + duration`; the sum is computed with
        /// `checked_add`, so a generated interval that would leave the `u64`
        /// range fails instead of wrapping.
        #[test]
        fn a_timelock_unlocks_after_its_start(
            (start, duration) in {
                let env = Env::default();
                (
                    soroban_timepoint(&env, 1, 2_000_000_000),
                    soroban_duration(&env, 0, 1_000_000_000),
                )
            },
        ) {
            let unlock = start
                .to_unix()
                .checked_add(duration.to_seconds())
                .expect("the bounded ranges cannot overflow u64");

            prop_assert!(unlock >= start.to_unix());
            prop_assert_eq!(unlock - start.to_unix(), duration.to_seconds());
            prop_assert!(start.to_unix() >= 1);
        }
    }
}
