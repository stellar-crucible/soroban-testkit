use core::sync::atomic::{AtomicU64, Ordering};

pub use crate::cost_model::{
    contract_cost_model, contract_cost_model_with_config, contract_cost_model_with_multipliers,
    ContractCostComponent, ContractCostEntry, ContractCostModel, ContractCostModelConfig,
    CostDimension, BASELINE_COST_ENTRIES,
};
pub use crate::crypto::{
    ed25519_signature_pair, malleated_signature, secp256k1_signature_pair, Ed25519SignaturePair,
    Secp256k1SignaturePair,
};
pub use crate::executables::{contract_executable, contract_executable_wasm_hash, deployer_salt};
pub use crate::storage_tier::{
    mixed_tier_allocations, mixed_tier_allocations_with_ttl, mixed_tier_key_pairs, storage_tier,
    storage_tier_with_default_ttl, storage_tier_with_ttl, MixedTierAllocation,
    MixedTierTtlAllocation, StorageTierWithTtl,
};

use proptest::prelude::*;
use soroban_sdk::xdr::{
    AccountId, ContractId, Hash, Limits, PublicKey, ScAddress, ScMap, ScMapEntry, ScVal, ScVec,
    Uint256, VecM, WriteXdr as _,
};
use soroban_sdk::{testutils::Address as _, Address, Env, IntoVal, Symbol, TryFromVal as _, Val};

/// Generate arbitrary Soroban invocation arguments, including values whose
/// object handles are owned by `env`. `typed` contains valid arguments for a
/// particular contract method; the strategy inserts zero or more rogue values
/// at arbitrary positions so the same client call exercises type validation.
///
/// The primitive branches shrink to zero, `false`, and the empty symbol. Host
/// objects and errors are included as type mismatches and disappear as the
/// list shrinks.
pub fn fuzzed_invocation_args(
    env: &Env,
    typed: Vec<Val>,
    max_rogue: u32,
) -> impl Strategy<Value = Vec<Val>> {
    let env = env.clone();
    let int_env = env.clone();
    let bool_env = env.clone();
    let uint_env = env.clone();
    let address_env = env.clone();
    let rogue = prop_oneof![
        4 => any::<i32>().prop_map(move |value| value.into_val(&int_env)),
        4 => any::<bool>().prop_map(move |value| value.into_val(&bool_env)),
        2 => any::<u32>().prop_map(move |value| value.into_val(&uint_env)),
        2 => Just(Symbol::new(&env, "rogue_symbol").into_val(&env)),
        1 => Just(soroban_sdk::Error::from_contract_error(1).into_val(&env)),
        1 => any::<u8>().prop_map(move |_| Address::generate(&address_env).into_val(&address_env)),
    ];
    (
        prop::collection::vec(rogue, 0..=max_rogue as usize),
        any::<Vec<usize>>(),
    )
        .prop_map(move |(mut rogue, order)| {
            let mut args = typed.clone();
            // Interleave rogue values at deterministic positions derived from
            // the order vector. This keeps the strategy shrinkable.
            for (index, value) in rogue.drain(..).enumerate() {
                let position = order.get(index).copied().unwrap_or(index).min(args.len());
                args.insert(position, value);
            }
            args
        })
}

/// The two address formats Soroban accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressType {
    /// An ed25519 account, the `G...` form.
    Account,
    /// A contract id, the `C...` form.
    Contract,
}

/// A generated Soroban address: 32 raw bytes tagged with the format they
/// belong to.
///
/// Both formats are opaque 32-byte values on the wire, so the tag is what
/// separates an ed25519 public key from a contract id. Turn it into a host
/// [`Address`] with [`AddressBytes::to_address`], or into its XDR form with
/// `ScAddress::from`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AddressBytes {
    address_type: AddressType,
    bytes: [u8; 32],
}

impl AddressBytes {
    /// Build an address from its format tag and bytes.
    ///
    /// Crate-internal: the public API generates addresses, and a hand-built one
    /// is only useful to a test that needs a predictable value.
    #[cfg(test)]
    pub(crate) fn from_parts(address_type: AddressType, bytes: [u8; 32]) -> Self {
        Self {
            address_type,
            bytes,
        }
    }

    /// The format this address was generated as.
    pub fn address_type(&self) -> AddressType {
        self.address_type
    }

    /// The raw 32 bytes, as an ed25519 public key or a contract id.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// The raw 32 bytes, consuming the address.
    pub fn into_bytes(self) -> [u8; 32] {
        self.bytes
    }

    /// Rebuild the address as a host value owned by `env`.
    pub fn to_address(self, env: &Env) -> Address {
        Address::try_from_val(env, &ScAddress::from(self))
            .expect("a 32-byte address is a valid ScAddress")
    }
}

impl From<AddressBytes> for ScAddress {
    fn from(address: AddressBytes) -> Self {
        match address.address_type {
            AddressType::Account => ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                Uint256(address.bytes),
            ))),
            AddressType::Contract => ScAddress::Contract(ContractId(Hash(address.bytes))),
        }
    }
}

/// Hands every generated address a distinct suffix, so two cases in one run
/// cannot collide even if their random prefixes do.
static NEXT_ADDRESS_NONCE: AtomicU64 = AtomicU64::new(1);

/// Generates either an account or a contract address.
pub fn address() -> impl Strategy<Value = AddressBytes> {
    prop_oneof![account_address(), contract_address()]
}

/// Generates an ed25519 account address.
pub fn account_address() -> impl Strategy<Value = AddressBytes> {
    address_of_type(AddressType::Account)
}

/// Generates a contract address.
pub fn contract_address() -> impl Strategy<Value = AddressBytes> {
    address_of_type(AddressType::Contract)
}

/// Generates an address of the requested format.
pub fn address_of_type(address_type: AddressType) -> impl Strategy<Value = AddressBytes> {
    any::<[u8; 32]>().prop_map(move |mut bytes| {
        let nonce = NEXT_ADDRESS_NONCE.fetch_add(1, Ordering::Relaxed);
        bytes[24..].copy_from_slice(&nonce.to_be_bytes());
        AddressBytes {
            address_type,
            bytes,
        }
    })
}

pub fn token_amount() -> impl Strategy<Value = i128> {
    0i128..=1_000_000_000_000_000i128
}

pub fn ledger_sequence() -> impl Strategy<Value = u32> {
    1u32..=10_000_000u32
}

pub fn timestamp() -> impl Strategy<Value = u64> {
    1_600_000_000u64..=2_000_000_000u64
}

/// The entry budget [`fuzzed_scval_map`] and [`fuzzed_scval_vec`] use when the
/// caller has no opinion: large enough to cross the host's element limits in a
/// handful of cases, small enough that printed payloads stay readable.
pub const DEFAULT_FUZZ_ENTRIES: u32 = 64;

/// A scalar a fuzzed map key or vector element can be built from.
///
/// Only the small, cheap variants are generated: a fuzz corpus is judged by
/// what the parser does with the *shape* of an object, and a nested tree three
/// objects deep says nothing a one-level object does not.
fn fuzz_scalar() -> impl Strategy<Value = ScVal> {
    prop_oneof![
        Just(ScVal::Void),
        any::<bool>().prop_map(ScVal::Bool),
        any::<u32>().prop_map(ScVal::U32),
        any::<i32>().prop_map(ScVal::I32),
        any::<u64>().prop_map(ScVal::U64),
        any::<i64>().prop_map(ScVal::I64),
    ]
}

/// A value a fuzzed map can carry, including the two null object forms.
///
/// `ScVal::Map(None)` and `ScVal::Vec(None)` are the XDR spellings of a null
/// object. A parser that unwraps `Option<ScMap>` without checking it is a real
/// deserialization bug, so the corpus includes them.
fn fuzz_value() -> impl Strategy<Value = ScVal> {
    prop_oneof![
        fuzz_scalar(),
        Just(ScVal::Vec(None)),
        Just(ScVal::Map(None)),
    ]
}

/// Unordered `(key, value)` pairs, bounded by `max_entries`.
fn fuzz_pairs(max_entries: u32) -> impl Strategy<Value = Vec<(ScVal, ScVal)>> {
    prop::collection::vec((fuzz_scalar(), fuzz_value()), 0..=max_entries as usize)
}

/// Builds an `ScMap`, ordering and de-duplicating the keys when `sorted`.
///
/// The unsorted branch keeps generation order and therefore keeps duplicate
/// keys too: both are shapes a contract parser has to reject rather than
/// assume away, and the host validates a `ScMap` before a contract ever sees
/// one.
fn sc_map(pairs: Vec<(ScVal, ScVal)>, sorted: bool) -> ScMap {
    let mut pairs = pairs;
    if sorted {
        pairs.sort_by(|left, right| left.0.cmp(&right.0));
        pairs.dedup_by(|left, right| left.0 == right.0);
    }
    let entries: Vec<ScMapEntry> = pairs
        .into_iter()
        .map(|(key, val)| ScMapEntry { key, val })
        .collect();
    ScMap(VecM::try_from(entries).expect("a generated map fits the XDR length bound"))
}

/// Generates an `ScVal::Map` with at most `max_entries` entries.
///
/// Half the cases are sorted and key-unique, which is the shape the host
/// accepts; the other half keep generation order, which is what a parser sees
/// when a caller hands it an XDR map that was never canonicalised. Shrinking
/// removes entries and simplifies keys, so a failing call shrinks towards the
/// smallest map that still fails.
///
/// ```text
/// proptest! {
///     #[test]
///     fn a_parser_never_panics_on_a_map(map in fuzzed_scval_map(16)) {
///         // map: ScVal::Map(Some(..)) bounded by 16 entries
///     }
/// }
/// ```
pub fn fuzzed_scval_map(max_entries: u32) -> impl Strategy<Value = ScVal> {
    (fuzz_pairs(max_entries), any::<bool>())
        .prop_map(|(pairs, sorted)| ScVal::Map(Some(sc_map(pairs, sorted))))
}

/// Generates an `ScVal::Vec` with at most `max_entries` elements.
///
/// Pass the host's element limit to stay inside it, or a multiple of it to
/// generate the oversized vectors a parser has to refuse instead of allocating
/// for. Elements include the null object forms, so a parser that unwraps
/// `Option<ScVec>` is exercised as well.
pub fn fuzzed_scval_vec(max_entries: u32) -> impl Strategy<Value = ScVal> {
    prop::collection::vec(fuzz_value(), 0..=max_entries as usize).prop_map(|items| {
        let vec = VecM::try_from(items).expect("a generated vector fits the XDR length bound");
        ScVal::Vec(Some(ScVec(vec)))
    })
}

/// Generates truncated XDR: the bytes of a real map or vector, cut at an
/// arbitrary offset.
///
/// A cut inside the four-byte type discriminant leaves a truncated tag; a cut
/// inside an entry leaves a length that promises more bytes than follow. Both
/// are what a contract sees when a caller sends a short payload, and neither
/// may panic a parser that returns an error instead.
///
/// The strategy is built to shrink: proptest shortens the cut towards zero, so
/// a failing deserialization reports the smallest prefix that still fails
/// rather than the whole object.
pub fn malformed_object_payload() -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![fuzzed_scval_map(4), fuzzed_scval_vec(4)]
        .prop_flat_map(|object| {
            let bytes = object
                .to_xdr(Limits::none())
                .expect("a generated ScVal encodes as XDR");
            let length = bytes.len();
            (Just(bytes), 0..length)
        })
        .prop_map(|(mut bytes, keep)| {
            bytes.truncate(keep);
            bytes
        })
}

#[cfg(test)]
mod tests {
    use super::{
        account_address, address, address_of_type, contract_address, ledger_sequence, timestamp,
        token_amount, AddressType,
    };
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::xdr::ScAddress;
    use soroban_sdk::Env;
    use std::collections::HashSet;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn token_amounts_stay_non_negative(amount in token_amount()) {
            prop_assert!(amount >= 0);
        }

        #[test]
        fn token_amounts_stay_below_the_supply_cap(amount in token_amount()) {
            prop_assert!(amount <= 1_000_000_000_000_000);
        }

        #[test]
        fn ledger_sequences_are_never_zero(seq in ledger_sequence()) {
            prop_assert!(seq >= 1);
            prop_assert!(seq <= 10_000_000);
        }

        #[test]
        fn timestamps_fall_inside_the_stellar_era(ts in timestamp()) {
            prop_assert!(ts >= 1_600_000_000);
            prop_assert!(ts <= 2_000_000_000);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn account_addresses_are_32_byte_ed25519_keys(address in account_address()) {
            prop_assert_eq!(address.address_type(), AddressType::Account);
            prop_assert_eq!(address.as_bytes().len(), 32);
            prop_assert!(matches!(ScAddress::from(address), ScAddress::Account(_)));
        }

        #[test]
        fn contract_addresses_are_32_byte_contract_ids(address in contract_address()) {
            prop_assert_eq!(address.address_type(), AddressType::Contract);
            prop_assert_eq!(address.as_bytes().len(), 32);
            prop_assert!(matches!(ScAddress::from(address), ScAddress::Contract(_)));
        }

        #[test]
        fn the_type_filter_only_yields_the_requested_format(address in address_of_type(AddressType::Contract)) {
            prop_assert_eq!(address.address_type(), AddressType::Contract);
        }

        #[test]
        fn the_default_strategy_yields_one_of_the_two_formats(address in address()) {
            prop_assert!(matches!(
                address.address_type(),
                AddressType::Account | AddressType::Contract
            ));
        }
    }

    #[test]
    fn generated_addresses_convert_to_host_addresses() {
        let env = Env::default();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(32));
        runner
            .run(
                &(account_address(), contract_address()),
                |(account, contract)| {
                    let account = ScAddress::from(account.to_address(&env));
                    let contract = ScAddress::from(contract.to_address(&env));
                    prop_assert!(matches!(account, ScAddress::Account(_)));
                    prop_assert!(matches!(contract, ScAddress::Contract(_)));
                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn a_single_run_never_generates_a_duplicate_address() {
        let mixed = prop_oneof![account_address(), contract_address()];
        let mut runner = TestRunner::new(ProptestConfig::with_cases(32));
        runner
            .run(&proptest::collection::vec(mixed, 64), |addresses| {
                let unique: HashSet<_> =
                    addresses.iter().map(|address| address.as_bytes()).collect();
                prop_assert_eq!(unique.len(), addresses.len());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn strategies_compose_into_tuples() {
        let mut runner = TestRunner::default();
        runner
            .run(&(token_amount(), ledger_sequence()), |(amount, seq)| {
                prop_assert!(amount >= 0);
                prop_assert!(seq >= 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn filtered_strategy_only_yields_sufficient_balances() {
        let strategy = (token_amount(), token_amount())
            .prop_filter("sender must have sufficient balance", |(a, b)| a >= b);

        let mut runner = TestRunner::default();
        runner
            .run(&strategy, |(sender_balance, amount)| {
                prop_assert!(sender_balance >= amount);
                Ok(())
            })
            .unwrap();
    }
}

#[cfg(test)]
mod object_fuzz_tests {
    use super::{fuzzed_scval_map, fuzzed_scval_vec, malformed_object_payload};
    use proptest::prelude::*;
    use proptest::test_runner::{TestError, TestRunner};
    use soroban_sdk::xdr::{Limits, ReadXdr as _, ScMap, ScVal, WriteXdr as _};

    /// The keys of a map in the order the ledger would hand them over.
    fn keys(map: &ScMap) -> Vec<&ScVal> {
        map.0.iter().map(|entry| &entry.key).collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn a_fuzzed_map_stays_within_its_entry_budget(map in fuzzed_scval_map(6)) {
            let ScVal::Map(Some(map)) = &map else {
                return Err(TestCaseError::fail("fuzzed_scval_map did not yield a map"));
            };

            prop_assert!(map.0.len() <= 6);

            // Whatever the ordering, the object is real XDR: the encoder and
            // decoder round-trip the same entries, so a parser is being handed
            // a payload it could receive on chain rather than random bytes.
            let wrapped = ScVal::Map(Some(map.clone()));
            let bytes = wrapped.to_xdr(Limits::none()).expect("a map encodes");
            prop_assert_eq!(
                ScVal::from_xdr(bytes.as_slice(), Limits::none()).expect("a map decodes"),
                wrapped
            );
        }

        #[test]
        fn a_fuzzed_vector_stays_within_its_entry_budget(vector in fuzzed_scval_vec(6)) {
            let ScVal::Vec(Some(vector)) = &vector else {
                return Err(TestCaseError::fail("fuzzed_scval_vec did not yield a vector"));
            };

            prop_assert!(vector.0.len() <= 6);

            let wrapped = ScVal::Vec(Some(vector.clone()));
            let bytes = wrapped.to_xdr(Limits::none()).expect("a vector encodes");
            prop_assert_eq!(
                ScVal::from_xdr(bytes.as_slice(), Limits::none()).expect("a vector decodes"),
                wrapped
            );
        }

        #[test]
        fn an_oversized_budget_generates_larger_vectors(vector in fuzzed_scval_vec(4_096)) {
            let ScVal::Vec(Some(vector)) = &vector else {
                return Err(TestCaseError::fail("fuzzed_scval_vec did not yield a vector"));
            };

            // The caller names the limit, so a test that wants the size a host
            // refuses asks for more entries than the host accepts.
            prop_assert!(vector.0.len() <= 4_096);
        }

        #[test]
        fn a_malformed_payload_never_decodes_as_a_complete_value(
            payload in malformed_object_payload()
        ) {
            prop_assert!(
                ScVal::from_xdr(payload.as_slice(), Limits::none()).is_err(),
                "a {} byte payload decoded even though it was truncated",
                payload.len()
            );
        }
    }

    #[test]
    fn the_map_strategy_produces_both_canonical_and_unsorted_maps() {
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        // `run` takes `Fn`, so the tallies live outside the closure.
        let sorted = std::cell::Cell::new(0usize);
        let unsorted = std::cell::Cell::new(0usize);

        runner
            .run(&fuzzed_scval_map(5), |value| {
                let ScVal::Map(Some(map)) = &value else {
                    return Ok(());
                };
                let keys = keys(map);
                if keys.windows(2).all(|pair| pair[0] < pair[1]) {
                    sorted.set(sorted.get() + 1);
                } else {
                    unsorted.set(unsorted.get() + 1);
                }
                Ok(())
            })
            .expect("the map strategy runs");

        // Both halves are reachable: a suite that only ever saw sorted maps
        // would never prove anything about a host that refuses the others.
        assert!(
            sorted.get() > 0,
            "no canonical map was generated in 64 cases"
        );
        assert!(
            unsorted.get() > 0,
            "no unsorted map was generated in 64 cases"
        );
    }

    #[test]
    fn an_unsorted_map_breaks_a_parser_that_assumes_ledger_order() {
        // This is the bug the strategy exists to find: a parser that assumes the
        // ledger handed it sorted keys and indexes into them. The corpus below
        // has to produce a map that violates the assumption, and proptest
        // reports the smallest one it found.
        // The failure is the point of this test, so there is nothing worth
        // persisting as a regression seed.
        let config = ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::with_cases(64)
        };
        let mut runner = TestRunner::new(config);
        let outcome = runner.run(&fuzzed_scval_map(5), |value| {
            let ScVal::Map(Some(map)) = &value else {
                return Ok(());
            };
            let keys = keys(map);
            prop_assert!(
                keys.windows(2).all(|pair| pair[0] < pair[1]),
                "keys arrived out of ledger order"
            );
            Ok(())
        });

        assert!(
            matches!(outcome, Err(TestError::Fail(..))),
            "the corpus never produced an unsorted map, so the property never failed"
        );
    }

    #[test]
    fn a_failing_deserialization_shrinks_towards_a_minimal_payload() {
        // A parser that insists on a complete value fails on every truncated
        // payload, and the reported case is the smallest prefix rather than the
        // object it was cut from.
        let config = ProptestConfig {
            failure_persistence: None,
            ..ProptestConfig::with_cases(64)
        };
        let mut runner = TestRunner::new(config);
        let outcome = runner.run(&malformed_object_payload(), |payload| {
            prop_assert!(
                ScVal::from_xdr(payload.as_slice(), Limits::none()).is_ok(),
                "{} byte payload was rejected",
                payload.len()
            );
            Ok(())
        });

        match outcome {
            Err(TestError::Fail(_, payload)) => assert!(
                payload.len() < 4,
                "the smallest failing payload should be a truncated tag, got {} bytes",
                payload.len()
            ),
            other => panic!("expected a shrunk counterexample, got {other:?}"),
        }
    }
}
