//! Strategies over contract data entry TTLs.
//!
//! Since protocol 20 every contract data entry carries a *live-until* ledger:
//! the last ledger on which it can still be read. Rent bugs live at the edges
//! of that interval — an entry renewed one ledger too late, a threshold that
//! never fires, an extension that overshoots the network maximum — so these
//! strategies generate the intervals themselves and shrink every failure
//! towards the expiration boundary.
//!
//! The vocabulary follows the SDK. The *TTL* of an entry is
//! `live_until_ledger - current_ledger`, the number returned by
//! `env.storage().persistent().get_ttl(&key)` and the unit `extend_ttl`'s
//! `threshold` and `extend_to` are measured in. An entry is live while its TTL
//! is non-negative, so a TTL of `0` means "readable this ledger, gone the next".

use proptest::prelude::*;

/// The largest TTL Mainnet lets an entry be extended to, in ledgers
/// (`3_110_400`, roughly 180 days at five-second closes).
///
/// This is the value to pass as `extend_to` at the ceiling, and the value
/// `env.ledger().set_max_entry_ttl(..)` takes in a test environment.
pub const MAINNET_MAX_ENTRY_TTL: u32 = 3_110_400;

/// A contract data entry's live interval, as seen from one ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryTtl {
    /// The ledger the test is running on.
    pub current_ledger: u32,
    /// The last ledger on which the entry is still readable.
    pub live_until_ledger: u32,
}

impl EntryTtl {
    /// Whether the entry can still be read on `current_ledger`.
    pub fn is_live(&self) -> bool {
        self.live_until_ledger >= self.current_ledger
    }

    /// The TTL the SDK would report — `live_until_ledger - current_ledger` —
    /// or `None` once the entry has expired.
    pub fn ttl(&self) -> Option<u32> {
        self.live_until_ledger.checked_sub(self.current_ledger)
    }

    /// How many ledgers ago the entry expired; `None` while it is live.
    ///
    /// `Some(1)` is the boundary case: the entry was readable on the previous
    /// ledger and is not on this one.
    pub fn ledgers_since_expiry(&self) -> Option<u32> {
        self.current_ledger
            .checked_sub(self.live_until_ledger)
            .filter(|&distance| distance > 0)
    }

    /// The first ledger on which the entry is no longer readable.
    pub fn expires_at(&self) -> u32 {
        self.live_until_ledger.saturating_add(1)
    }

    /// The same entry, observed `ledgers` later.
    pub fn advanced_by(&self, ledgers: u32) -> Self {
        Self {
            current_ledger: self.current_ledger.saturating_add(ledgers),
            live_until_ledger: self.live_until_ledger,
        }
    }
}

/// Generates a live entry on `current_ledger` whose TTL lies in
/// `min_threshold..=max_ttl`.
///
/// `min_threshold` is usually the rent threshold a contract renews at (pass
/// `0` to include entries on their very last readable ledger), and `max_ttl`
/// the network ceiling a host will extend to — [`MAINNET_MAX_ENTRY_TTL`] on
/// Mainnet, or whatever the test passed to `set_max_entry_ttl`. A `max_ttl`
/// that would push `live_until_ledger` past `u32::MAX` is clamped.
///
/// Shrinking moves the TTL towards `min_threshold`, so a renewal bug
/// minimises to the entry closest to expiring rather than to an arbitrary
/// interval.
///
/// # Panics
///
/// Panics when `min_threshold > max_ttl`, since no interval satisfies both.
pub fn entry_ttl(
    current_ledger: u32,
    min_threshold: u32,
    max_ttl: u32,
) -> impl Strategy<Value = EntryTtl> {
    assert!(
        min_threshold <= max_ttl,
        "entry_ttl: min_threshold ({min_threshold}) is above max_ttl ({max_ttl}), so no live interval exists"
    );
    let headroom = u32::MAX - current_ledger;
    let low = min_threshold.min(headroom);
    let high = max_ttl.min(headroom);

    (low..=high).prop_map(move |ttl| EntryTtl {
        current_ledger,
        live_until_ledger: current_ledger + ttl,
    })
}

/// Generates an entry that has already expired by `current_ledger`: its
/// `live_until_ledger` is anywhere from ledger `0` to `current_ledger - 1`.
///
/// Shrinking moves `live_until_ledger` up towards `current_ledger - 1`, the
/// entry that expired one ledger ago, which is the case an off-by-one in a
/// liveness check gets wrong.
///
/// # Panics
///
/// Panics when `current_ledger` is `0`, since nothing can have expired before
/// the first ledger.
pub fn expired_ttl(current_ledger: u32) -> impl Strategy<Value = EntryTtl> {
    assert!(
        current_ledger > 0,
        "expired_ttl: nothing can have expired before ledger 0; pass a current_ledger of at least 1"
    );
    // Generate the distance past expiry so proptest's shrink-towards-the-low-
    // end lands on the boundary.
    (1..=current_ledger).prop_map(move |since_expiry| EntryTtl {
        current_ledger,
        live_until_ledger: current_ledger - since_expiry,
    })
}

/// Generates a TTL extension — the number of ledgers a bump adds, or the
/// number of ledgers a test advances between renewals — in
/// `1..=MAINNET_MAX_ENTRY_TTL`.
///
/// Shrinks towards `1`, the smallest step that can cross a boundary.
pub fn ttl_extension_delta() -> impl Strategy<Value = u32> {
    ttl_extension_delta_up_to(MAINNET_MAX_ENTRY_TTL)
}

/// [`ttl_extension_delta`] with a caller-chosen ceiling, for a test
/// environment configured with a smaller `max_entry_ttl`.
///
/// # Panics
///
/// Panics when `max_delta` is `0`.
pub fn ttl_extension_delta_up_to(max_delta: u32) -> impl Strategy<Value = u32> {
    assert!(
        max_delta > 0,
        "ttl_extension_delta_up_to: max_delta must be at least 1"
    );
    1..=max_delta
}

#[cfg(test)]
mod tests {
    use super::{
        entry_ttl, expired_ttl, ttl_extension_delta, ttl_extension_delta_up_to, EntryTtl,
        MAINNET_MAX_ENTRY_TTL,
    };
    use proptest::prelude::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::testutils::storage::Persistent as _;
    use soroban_sdk::testutils::Ledger as _;
    use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Symbol};

    const KEY: Symbol = symbol_short!("balance");
    /// Renew when fewer than this many ledgers remain...
    const RENEW_THRESHOLD: u32 = 17_280;
    /// ...back up to this many.
    const RENEW_TO: u32 = 518_400;
    /// The ceiling the test environment is configured with.
    const TEST_MAX_TTL: u32 = 1_000_000;

    #[contract]
    pub struct Rent;

    #[contractimpl]
    impl Rent {
        pub fn put(env: Env, temporary: bool) {
            if temporary {
                env.storage().temporary().set(&KEY, &1u32);
            } else {
                env.storage().persistent().set(&KEY, &1u32);
            }
        }

        pub fn extend(env: Env, temporary: bool, to: u32) {
            if temporary {
                env.storage().temporary().extend_ttl(&KEY, to, to);
            } else {
                env.storage().persistent().extend_ttl(&KEY, to, to);
            }
        }

        /// The storage rent renewal function under test.
        pub fn renew(env: Env) {
            env.storage()
                .persistent()
                .extend_ttl(&KEY, RENEW_THRESHOLD, RENEW_TO);
        }

        pub fn has_temporary(env: Env) -> bool {
            env.storage().temporary().has(&KEY)
        }
    }

    /// An environment holding `KEY` with exactly the generated live interval.
    ///
    /// The entry is written on the ledger the interval is measured from with a
    /// minimum TTL of zero, then extended to the generated TTL, so the host's
    /// own bookkeeping — not the test — sets `live_until_ledger`.
    fn env_with(entry: EntryTtl, temporary: bool) -> (Env, Address) {
        let env = Env::default();
        env.ledger().set_max_entry_ttl(TEST_MAX_TTL);
        env.ledger().set_min_persistent_entry_ttl(1);
        env.ledger().set_min_temp_entry_ttl(1);

        let written_at = entry.current_ledger.min(entry.live_until_ledger);
        env.ledger().set_sequence_number(written_at);
        let contract = env.register(Rent, ());
        let client = RentClient::new(&env, &contract);
        client.put(&temporary);
        client.extend(&temporary, &(entry.live_until_ledger - written_at));

        env.ledger().set_sequence_number(entry.current_ledger);
        (env, contract)
    }

    fn persistent_ttl(env: &Env, contract: &Address) -> u32 {
        env.as_contract(contract, || env.storage().persistent().get_ttl(&KEY))
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        #[test]
        fn generated_live_intervals_are_what_the_host_reports(
            entry in entry_ttl(1_000, 0, TEST_MAX_TTL - 1_000),
        ) {
            prop_assert!(entry.is_live());
            let (env, contract) = env_with(entry, false);

            prop_assert_eq!(Some(persistent_ttl(&env, &contract)), entry.ttl());
        }

        #[test]
        fn renewal_keeps_every_live_entry_above_its_threshold(
            entry in (1u32..100_000)
                .prop_flat_map(|current| entry_ttl(current, 0, TEST_MAX_TTL - 100_000)),
        ) {
            let (env, contract) = env_with(entry, false);
            let before = persistent_ttl(&env, &contract);

            RentClient::new(&env, &contract).renew();
            let after = persistent_ttl(&env, &contract);

            prop_assert!(after >= RENEW_THRESHOLD, "ttl {after} after renewing from {before}");
            if before < RENEW_THRESHOLD {
                prop_assert_eq!(after, RENEW_TO);
            } else {
                prop_assert_eq!(after, before);
            }
        }

        #[test]
        fn renewing_before_each_gap_keeps_the_entry_alive(
            entry in entry_ttl(500, RENEW_THRESHOLD, RENEW_TO),
            gaps in prop::collection::vec(ttl_extension_delta_up_to(RENEW_THRESHOLD), 1..6),
        ) {
            let (env, contract) = env_with(entry, false);
            let client = RentClient::new(&env, &contract);

            for gap in gaps {
                let remaining = persistent_ttl(&env, &contract);
                prop_assert!(gap <= remaining, "gap {gap} would outrun ttl {remaining}");
                env.ledger().set_sequence_number(env.ledger().sequence() + gap);
                client.renew();
                prop_assert!(persistent_ttl(&env, &contract) >= RENEW_THRESHOLD);
            }
        }

        #[test]
        fn an_expired_temporary_entry_is_gone(entry in expired_ttl(50_000)) {
            prop_assert!(!entry.is_live());
            prop_assert_eq!(entry.ttl(), None);
            let (env, contract) = env_with(entry, true);

            prop_assert!(!RentClient::new(&env, &contract).has_temporary());
        }

        #[test]
        fn a_live_temporary_entry_is_present_up_to_its_last_ledger(
            entry in entry_ttl(50_000, 0, 10_000),
        ) {
            let (env, contract) = env_with(entry, true);
            let client = RentClient::new(&env, &contract);
            prop_assert!(client.has_temporary());

            env.ledger().set_sequence_number(entry.live_until_ledger);
            prop_assert!(client.has_temporary());
            env.ledger().set_sequence_number(entry.expires_at());
            prop_assert!(!client.has_temporary());
        }

        #[test]
        fn extension_deltas_stay_inside_the_network_ceiling(delta in ttl_extension_delta()) {
            prop_assert!((1..=MAINNET_MAX_ENTRY_TTL).contains(&delta));
        }

        #[test]
        fn advancing_by_more_than_the_ttl_expires_the_entry(
            entry in entry_ttl(10, 0, 5_000),
            extra in ttl_extension_delta_up_to(5_000),
        ) {
            let ttl = entry.ttl().unwrap();
            let later = entry.advanced_by(ttl + extra);

            prop_assert!(!later.is_live());
            prop_assert_eq!(later.ledgers_since_expiry(), Some(extra));
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
    fn a_failing_live_interval_shrinks_to_the_threshold() {
        // A renewal that wrongly ignores any entry with less than 30 days left.
        let smallest = minimise(entry_ttl(1_000, RENEW_THRESHOLD, RENEW_TO), |entry| {
            entry.ttl().unwrap() < 518_000
        });

        assert_eq!(smallest.ttl(), Some(RENEW_THRESHOLD));
    }

    #[test]
    fn a_failing_expired_interval_shrinks_to_one_ledger_past_expiry() {
        let smallest = minimise(expired_ttl(80_000), |entry| !entry.is_live());

        assert_eq!(smallest.live_until_ledger, 79_999);
        assert_eq!(smallest.ledgers_since_expiry(), Some(1));
    }

    #[test]
    fn a_failing_extension_shrinks_to_one_ledger() {
        assert_eq!(minimise(ttl_extension_delta(), |_| true), 1);
    }

    #[test]
    fn a_zero_threshold_includes_the_last_readable_ledger() {
        let smallest = minimise(entry_ttl(7, 0, 100), |_| true);

        assert_eq!(smallest.ttl(), Some(0));
        assert!(smallest.is_live());
        assert_eq!(smallest.expires_at(), 8);
    }

    #[test]
    fn intervals_near_the_top_of_the_ledger_range_are_clamped() {
        let mut runner = TestRunner::deterministic();
        for _ in 0..64 {
            let entry = entry_ttl(u32::MAX - 10, 0, MAINNET_MAX_ENTRY_TTL)
                .new_tree(&mut runner)
                .unwrap()
                .current();
            assert!(entry.ttl().unwrap() <= 10);
        }
    }

    #[test]
    #[should_panic(expected = "min_threshold (10) is above max_ttl (5)")]
    fn an_empty_live_range_is_refused() {
        let _ = entry_ttl(0, 10, 5);
    }

    #[test]
    #[should_panic(expected = "nothing can have expired before ledger 0")]
    fn nothing_expires_before_the_first_ledger() {
        let _ = expired_ttl(0);
    }
}
