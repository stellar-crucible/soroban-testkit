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

/// The network's nominal ledger close interval: Stellar produces a ledger about
/// every five seconds.
pub const SECONDS_PER_LEDGER: u64 = 5;

/// The earliest timestamp [`realistic_ledger_window`] starts from — a Stellar
/// mainnet-era moment rather than the unix epoch.
pub const MIN_LEDGER_TIMESTAMP: u64 = 1_600_000_000;

/// The latest timestamp [`realistic_ledger_window`] starts from.
pub const MAX_LEDGER_TIMESTAMP: u64 = 1_900_000_000;

/// The widest sequence span [`realistic_ledger_window`] generates, about six
/// days of network time at [`SECONDS_PER_LEDGER`].
pub const MAX_LEDGER_WINDOW_LEDGERS: u32 = 100_000;

/// A sequence window and the timestamps that go with it.
///
/// Both ends are inclusive: `start_sequence`/`start_timestamp` describe the
/// ledger the window opens on, and `end_sequence`/`end_timestamp` the ledger it
/// closes on. A window with an equal start and end is *immediate* — no ledger
/// closes and no time passes between the two observations.
///
/// [`realistic_ledger_window`] keeps timestamps correlated with sequences:
/// `elapsed_secs()` is `ledger_count() * SECONDS_PER_LEDGER` plus a bounded
/// clock skew, so moving the sequence forward always moves the clock forward by
/// roughly the network's five-second cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerWindow {
    /// The first ledger in the window.
    pub start_sequence: u32,
    /// The last ledger in the window; never below `start_sequence`.
    pub end_sequence: u32,
    /// The unix timestamp of `start_sequence`.
    pub start_timestamp: u64,
    /// The unix timestamp of `end_sequence`.
    pub end_timestamp: u64,
    /// Extra seconds the window took beyond the nominal close cadence, simulating
    /// a validator clock that drifts from ledger production.
    pub skew_secs: u64,
}

impl LedgerWindow {
    /// How many ledgers the window spans; zero for an immediate window.
    pub fn ledger_count(&self) -> u32 {
        self.end_sequence - self.start_sequence
    }

    /// How much network time the window spans, clock skew included.
    pub fn elapsed_secs(&self) -> u64 {
        self.end_timestamp - self.start_timestamp
    }

    /// Whether the window closes on the same ledger it opens on.
    pub fn is_immediate(&self) -> bool {
        self.start_sequence == self.end_sequence && self.start_timestamp == self.end_timestamp
    }

    /// The timestamp at `sequence`, interpolated across the window.
    ///
    /// `sequence` is clamped to the window, so the start returns
    /// `start_timestamp` and the end returns `end_timestamp`. The interpolation
    /// spreads the window's elapsed time (skew included) linearly, which is the
    /// clock a contract observes at an intermediate ledger.
    pub fn timestamp_at(&self, sequence: u32) -> u64 {
        let span = self.ledger_count();
        if span == 0 {
            return self.start_timestamp;
        }
        let sequence = sequence.clamp(self.start_sequence, self.end_sequence);
        let elapsed = self.elapsed_secs();
        self.start_timestamp + elapsed * u64::from(sequence - self.start_sequence) / u64::from(span)
    }

    /// The ledger whose timestamp is at or immediately before `timestamp`,
    /// clamped to the window.
    ///
    /// This is the inverse of [`Self::timestamp_at`] at the endpoints: a
    /// timestamp before the window reads `start_sequence`, and one at or after
    /// the end reads `end_sequence`.
    pub fn sequence_at(&self, timestamp: u64) -> u32 {
        let elapsed = self.elapsed_secs();
        if elapsed == 0 || timestamp <= self.start_timestamp {
            return self.start_sequence;
        }
        if timestamp >= self.end_timestamp {
            return self.end_sequence;
        }
        let offset = timestamp - self.start_timestamp;
        self.start_sequence
            + u32::try_from(offset * u64::from(self.ledger_count()) / elapsed).unwrap_or(u32::MAX)
    }
}

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

/// Generates a realistic ledger window: a sequence span plus the timestamps at
/// its two ends, correlated at the network's five-second close cadence.
///
/// The start sequence lies in the same range
/// [`ledger_sequence`](crate::strategies::ledger_sequence) draws from, the span
/// covers anything from an immediate observation to
/// [`MAX_LEDGER_WINDOW_LEDGERS`], and the start timestamp lies in a realistic
/// mainnet era. `end_timestamp` is the start plus `span * SECONDS_PER_LEDGER`
/// plus a clock skew of at most one second per ledger, so the timestamps always
/// move with the sequence numbers:
///
/// ```
/// use proptest::strategy::{Strategy as _, ValueTree};
/// use proptest::test_runner::TestRunner;
/// use soroban_testkit_generators::temporal::realistic_ledger_window;
///
/// let mut runner = TestRunner::deterministic();
/// let window = realistic_ledger_window()
///     .new_tree(&mut runner)
///     .unwrap()
///     .current();
/// assert!(window.start_sequence <= window.end_sequence);
/// assert!(window.elapsed_secs() >= u64::from(window.ledger_count()) * 5);
/// ```
///
/// Shrinking moves the window towards immediate execution: the start sequence
/// to the lowest one, the span and the clock skew to zero, and the start
/// timestamp to [`MIN_LEDGER_TIMESTAMP`].
pub fn realistic_ledger_window() -> impl Strategy<Value = LedgerWindow> {
    (
        1u32..=(MAX_LEDGER_SEQUENCE - MAX_LEDGER_WINDOW_LEDGERS),
        0u32..=MAX_LEDGER_WINDOW_LEDGERS,
    )
        .prop_flat_map(|(start_sequence, ledger_count)| {
            (
                Just(start_sequence),
                Just(ledger_count),
                MIN_LEDGER_TIMESTAMP..=MAX_LEDGER_TIMESTAMP,
                // At most a second of drift per ledger, so the window averages
                // between five and six seconds per close.
                0u64..=u64::from(ledger_count),
            )
        })
        .prop_map(
            |(start_sequence, ledger_count, start_timestamp, skew_secs)| {
                let end_timestamp = start_timestamp
                    .saturating_add(u64::from(ledger_count) * SECONDS_PER_LEDGER)
                    .saturating_add(skew_secs);
                LedgerWindow {
                    start_sequence,
                    end_sequence: start_sequence + ledger_count,
                    start_timestamp,
                    end_timestamp,
                    skew_secs,
                }
            },
        )
}

#[cfg(test)]
mod tests {
    use super::{
        ledger_sequence_interval, realistic_ledger_window, soroban_duration, soroban_timepoint,
        LedgerWindow, MAX_LEDGER_SEQUENCE, SECONDS_PER_LEDGER,
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
    fn a_failing_window_shrinks_to_immediate_execution() {
        let smallest = minimise(realistic_ledger_window(), |_| true);
        assert!(
            smallest.is_immediate(),
            "a failed window should minimise to immediate execution, got {smallest:?}"
        );
    }

    #[test]
    fn windows_interpolate_between_their_endpoints() {
        let window = LedgerWindow {
            start_sequence: 100,
            end_sequence: 110,
            start_timestamp: 1_700_000_000,
            end_timestamp: 1_700_000_050,
            skew_secs: 0,
        };

        assert_eq!(window.ledger_count(), 10);
        assert_eq!(window.elapsed_secs(), 50);
        assert_eq!(window.timestamp_at(100), 1_700_000_000);
        assert_eq!(window.timestamp_at(105), 1_700_000_025);
        assert_eq!(window.timestamp_at(110), 1_700_000_050);
        assert_eq!(window.sequence_at(1_700_000_025), 105);
        assert_eq!(window.sequence_at(1_700_000_000), 100);
        assert_eq!(window.sequence_at(1_700_000_999), 110);
        assert!(!window.is_immediate());
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

        /// The timestamp window follows the sequence window at the network's
        /// close cadence, plus at most a second of drift per ledger.
        #[test]
        fn windows_keep_timestamps_correlated_with_sequences(
            window in realistic_ledger_window()
        ) {
            prop_assert!(window.start_sequence <= window.end_sequence);
            prop_assert_eq!(
                window.elapsed_secs(),
                u64::from(window.ledger_count()) * SECONDS_PER_LEDGER + window.skew_secs
            );
            prop_assert!(window.skew_secs <= u64::from(window.ledger_count()));
            prop_assert_eq!(window.timestamp_at(window.start_sequence), window.start_timestamp);
            prop_assert_eq!(window.timestamp_at(window.end_sequence), window.end_timestamp);

            // Timestamps never run backwards as the ledger advances.
            let mut previous = window.timestamp_at(window.start_sequence);
            for offset in 1..=window.ledger_count() {
                let current = window.timestamp_at(window.start_sequence + offset);
                prop_assert!(current >= previous);
                previous = current;
            }
        }

        /// A timelock that unlocks `lock_secs` after the window opens: the
        /// ledger the clock maps to must not be ahead of the unlock time, and
        /// the endpoint clocks stay ordered around it.
        #[test]
        fn fuzzing_a_timelock_against_the_ledger_window(
            window in realistic_ledger_window(),
            lock_secs in 0u64..=600,
        ) {
            let unlock_at = window.start_timestamp.saturating_add(lock_secs);
            let unlock_sequence = window.sequence_at(unlock_at);

            prop_assert!(unlock_sequence >= window.start_sequence);
            prop_assert!(unlock_sequence <= window.end_sequence);
            prop_assert!(window.timestamp_at(unlock_sequence) <= unlock_at);
            if unlock_at < window.end_timestamp {
                prop_assert!(window.timestamp_at(window.sequence_at(unlock_at) + 1) >= unlock_at);
            }
        }

        /// Linear vesting over the window's own clock: vested value is monotone
        /// in the sequence, bounded by the amount, and complete at the end.
        #[test]
        fn fuzzing_a_vesting_schedule_against_the_ledger_window(
            window in realistic_ledger_window(),
            amount in 0u128..=1_000_000_000_000_000_000u128,
        ) {
            fn vested(window: &LedgerWindow, amount: u128, sequence: u32) -> u128 {
                let total = window.elapsed_secs();
                if total == 0 {
                    return amount;
                }
                let elapsed = window.timestamp_at(sequence) - window.start_timestamp;
                amount * u128::from(elapsed) / u128::from(total)
            }

            let mut previous = 0u128;
            for offset in 0..=window.ledger_count() {
                let current = vested(&window, amount, window.start_sequence + offset);
                prop_assert!(current >= previous);
                prop_assert!(current <= amount);
                previous = current;
            }
            prop_assert_eq!(vested(&window, amount, window.end_sequence), amount);
        }

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
