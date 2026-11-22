//! [`arbitrary::Arbitrary`] implementations for Soroban test values.
//!
//! Coverage-guided fuzzers such as `cargo-fuzz` (libFuzzer) and `afl.rs` hand a
//! harness raw bytes and expect it to build typed inputs with the
//! [`arbitrary`](https://docs.rs/arbitrary) crate. This module gives those
//! harnesses the same value ranges the `proptest` strategies in
//! [`strategies`](crate::strategies) produce, so a property can be checked by
//! either ecosystem without restating what "valid" means.
//!
//! Enable it with the `arbitrary` feature. It does not need the `proptest`
//! feature:
//!
//! ```toml
//! soroban-testkit-generators = { version = "0.4", default-features = false, features = ["arbitrary"] }
//! ```
//!
//! Every implementation draws its values with
//! [`Unstructured::int_in_range`] or by copying fixed-size byte arrays, both of
//! which degrade to the low end of the range when the input runs out. A
//! harness therefore never sees a panic or an out-of-range value, even on an
//! empty or truncated input.
//!
//! ```
//! use arbitrary::{Arbitrary, Unstructured};
//! use soroban_testkit_generators::arbitrary::{ArbitraryLedgerInfo, TokenAmount};
//!
//! let mut input = Unstructured::new(&[7u8; 128]);
//! let amount = TokenAmount::arbitrary(&mut input).unwrap();
//! let ledger = ArbitraryLedgerInfo::arbitrary(&mut input).unwrap();
//! assert!(amount.get() >= 0);
//! assert!(ledger.get().max_entry_ttl >= ledger.get().min_persistent_entry_ttl);
//! ```

use ::arbitrary::{size_hint, Arbitrary, Result, Unstructured};
use soroban_sdk::testutils::{Ledger as _, LedgerInfo};
use soroban_sdk::Env;

pub use crate::types::{
    AddressBytes, AddressType, LEDGER_SEQUENCE_MAX, LEDGER_SEQUENCE_MIN, TIMESTAMP_MAX,
    TIMESTAMP_MIN, TOKEN_AMOUNT_MAX,
};

/// Protocol version every [`ArbitraryLedgerInfo`] reports: the version the
/// pinned `soroban-sdk` test host runs by default.
pub const PROTOCOL_VERSION: u32 = 28;

/// Smallest TTL, in ledgers, a generated temporary entry is guaranteed.
pub const MIN_TEMP_ENTRY_TTL_MIN: u32 = 1;
/// Largest minimum temporary-entry TTL a generated ledger uses (one day).
pub const MIN_TEMP_ENTRY_TTL_MAX: u32 = 17_280;
/// Smallest minimum persistent-entry TTL a generated ledger uses.
pub const MIN_PERSISTENT_ENTRY_TTL_MIN: u32 = 1;
/// Largest minimum persistent-entry TTL a generated ledger uses (30 days).
pub const MIN_PERSISTENT_ENTRY_TTL_MAX: u32 = 518_400;
/// Largest `max_entry_ttl` a generated ledger uses: the SDK test default.
pub const MAX_ENTRY_TTL_MAX: u32 = 6_312_000;
/// Largest base reserve, in stroops, a generated ledger uses.
pub const BASE_RESERVE_MAX: u32 = 100_000_000;

impl<'a> Arbitrary<'a> for AddressType {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        Ok(if bool::arbitrary(u)? {
            AddressType::Contract
        } else {
            AddressType::Account
        })
    }

    fn size_hint(depth: usize) -> (usize, Option<usize>) {
        bool::size_hint(depth)
    }
}

impl<'a> Arbitrary<'a> for AddressBytes {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let address_type = AddressType::arbitrary(u)?;
        let bytes = <[u8; 32]>::arbitrary(u)?;
        Ok(AddressBytes::new(address_type, bytes))
    }

    fn size_hint(depth: usize) -> (usize, Option<usize>) {
        size_hint::and(AddressType::size_hint(depth), <[u8; 32]>::size_hint(depth))
    }
}

macro_rules! bounded_newtype {
    (
        $(#[$meta:meta])*
        $name:ident($inner:ty), $min:expr, $max:expr
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($inner);

        impl $name {
            /// Smallest value this type can hold.
            pub const MIN: $inner = $min;
            /// Largest value this type can hold.
            pub const MAX: $inner = $max;

            /// Wrap `value`, or `None` when it falls outside
            /// `Self::MIN..=Self::MAX`.
            pub fn new(value: $inner) -> Option<Self> {
                (Self::MIN..=Self::MAX).contains(&value).then_some(Self(value))
            }

            /// The wrapped value.
            pub fn get(self) -> $inner {
                self.0
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> $inner {
                value.0
            }
        }

        impl<'a> Arbitrary<'a> for $name {
            fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
                Ok(Self(u.int_in_range(Self::MIN..=Self::MAX)?))
            }

            fn size_hint(depth: usize) -> (usize, Option<usize>) {
                <$inner>::size_hint(depth)
            }
        }
    };
}

bounded_newtype! {
    /// A token amount in `0..=TOKEN_AMOUNT_MAX`, the range
    /// [`strategies::token_amount`](crate::strategies) generates.
    TokenAmount(i128), 0, TOKEN_AMOUNT_MAX
}

bounded_newtype! {
    /// A ledger sequence in `LEDGER_SEQUENCE_MIN..=LEDGER_SEQUENCE_MAX`.
    LedgerSequence(u32), LEDGER_SEQUENCE_MIN, LEDGER_SEQUENCE_MAX
}

bounded_newtype! {
    /// A unix timestamp in `TIMESTAMP_MIN..=TIMESTAMP_MAX`.
    Timestamp(u64), TIMESTAMP_MIN, TIMESTAMP_MAX
}

/// A [`LedgerInfo`] the test host accepts.
///
/// The generated ledger always satisfies the invariants the host relies on:
///
/// - `protocol_version` is [`PROTOCOL_VERSION`];
/// - `sequence_number` is a [`LedgerSequence`] and `timestamp` a [`Timestamp`];
/// - both minimum TTLs are at least one ledger;
/// - `max_entry_ttl` is at least as large as both minimum TTLs, so every
///   `extend_ttl` the host may be asked for has room;
/// - `sequence_number + max_entry_ttl` cannot overflow `u32`.
///
/// Apply it to an environment with [`ArbitraryLedgerInfo::apply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArbitraryLedgerInfo(LedgerInfo);

impl ArbitraryLedgerInfo {
    /// The generated ledger.
    pub fn get(&self) -> &LedgerInfo {
        &self.0
    }

    /// The generated ledger, consuming the wrapper.
    pub fn into_inner(self) -> LedgerInfo {
        self.0
    }

    /// Install the generated ledger on `env`.
    pub fn apply(&self, env: &Env) {
        env.ledger().set(self.0.clone());
    }
}

impl From<ArbitraryLedgerInfo> for LedgerInfo {
    fn from(value: ArbitraryLedgerInfo) -> Self {
        value.0
    }
}

impl<'a> Arbitrary<'a> for ArbitraryLedgerInfo {
    fn arbitrary(u: &mut Unstructured<'a>) -> Result<Self> {
        let sequence_number = LedgerSequence::arbitrary(u)?.get();
        let timestamp = Timestamp::arbitrary(u)?.get();
        let network_id = <[u8; 32]>::arbitrary(u)?;
        let base_reserve = u.int_in_range(0..=BASE_RESERVE_MAX)?;
        let min_temp_entry_ttl = u.int_in_range(MIN_TEMP_ENTRY_TTL_MIN..=MIN_TEMP_ENTRY_TTL_MAX)?;
        let min_persistent_entry_ttl =
            u.int_in_range(MIN_PERSISTENT_ENTRY_TTL_MIN..=MIN_PERSISTENT_ENTRY_TTL_MAX)?;
        let floor = min_temp_entry_ttl.max(min_persistent_entry_ttl);
        let max_entry_ttl = u.int_in_range(floor..=MAX_ENTRY_TTL_MAX)?;
        Ok(Self(LedgerInfo {
            protocol_version: PROTOCOL_VERSION,
            sequence_number,
            timestamp,
            network_id,
            base_reserve,
            min_temp_entry_ttl,
            min_persistent_entry_ttl,
            max_entry_ttl,
        }))
    }

    fn size_hint(depth: usize) -> (usize, Option<usize>) {
        size_hint::and_all(&[
            u32::size_hint(depth),
            u64::size_hint(depth),
            <[u8; 32]>::size_hint(depth),
            u32::size_hint(depth),
            u32::size_hint(depth),
            u32::size_hint(depth),
            u32::size_hint(depth),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(seed: u64, len: usize) -> std::vec::Vec<u8> {
        // xorshift: deterministic, dependency-free input bytes.
        let mut state = seed.max(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    #[test]
    fn empty_input_yields_the_low_end_of_every_range() {
        let mut u = Unstructured::new(&[]);
        assert_eq!(TokenAmount::arbitrary(&mut u).unwrap().get(), 0);
        assert_eq!(
            LedgerSequence::arbitrary(&mut u).unwrap().get(),
            LEDGER_SEQUENCE_MIN
        );
        assert_eq!(Timestamp::arbitrary(&mut u).unwrap().get(), TIMESTAMP_MIN);
        let info = ArbitraryLedgerInfo::arbitrary(&mut u).unwrap();
        assert_eq!(info.get().protocol_version, PROTOCOL_VERSION);
        assert!(info.get().min_temp_entry_ttl >= 1);
    }

    #[test]
    fn generated_values_always_respect_their_bounds() {
        for seed in 0..512 {
            let data = bytes(seed, 256);
            let mut u = Unstructured::new(&data);
            let amount = TokenAmount::arbitrary(&mut u).unwrap();
            assert!((0..=TOKEN_AMOUNT_MAX).contains(&amount.get()));
            let seq = LedgerSequence::arbitrary(&mut u).unwrap();
            assert!((LEDGER_SEQUENCE_MIN..=LEDGER_SEQUENCE_MAX).contains(&seq.get()));
            let ts = Timestamp::arbitrary(&mut u).unwrap();
            assert!((TIMESTAMP_MIN..=TIMESTAMP_MAX).contains(&ts.get()));
            let info = ArbitraryLedgerInfo::arbitrary(&mut u).unwrap().into_inner();
            assert!(info.max_entry_ttl >= info.min_temp_entry_ttl);
            assert!(info.max_entry_ttl >= info.min_persistent_entry_ttl);
            assert!(info
                .sequence_number
                .checked_add(info.max_entry_ttl)
                .is_some());
        }
    }

    #[test]
    fn generated_addresses_become_host_addresses() {
        let env = Env::default();
        for seed in 0..64 {
            let data = bytes(seed, 64);
            let mut u = Unstructured::new(&data);
            let address = AddressBytes::arbitrary(&mut u).unwrap();
            let _ = address.to_address(&env);
        }
    }

    #[test]
    fn both_address_formats_are_reachable() {
        let mut seen_account = false;
        let mut seen_contract = false;
        for seed in 0..64 {
            let data = bytes(seed, 64);
            match AddressType::arbitrary(&mut Unstructured::new(&data)).unwrap() {
                AddressType::Account => seen_account = true,
                AddressType::Contract => seen_contract = true,
            }
        }
        assert!(seen_account && seen_contract);
    }

    #[test]
    fn new_rejects_out_of_range_values() {
        assert!(TokenAmount::new(-1).is_none());
        assert!(TokenAmount::new(TOKEN_AMOUNT_MAX + 1).is_none());
        assert!(LedgerSequence::new(0).is_none());
        assert_eq!(
            Timestamp::new(TIMESTAMP_MIN).map(Timestamp::get),
            Some(TIMESTAMP_MIN)
        );
    }

    #[test]
    fn a_generated_ledger_can_be_installed_on_an_env() {
        for seed in 0..32 {
            let env = Env::default();
            let data = bytes(seed, 128);
            let info = ArbitraryLedgerInfo::arbitrary(&mut Unstructured::new(&data)).unwrap();
            info.apply(&env);
            assert_eq!(env.ledger().sequence(), info.get().sequence_number);
            assert_eq!(env.ledger().timestamp(), info.get().timestamp);
        }
    }
}
