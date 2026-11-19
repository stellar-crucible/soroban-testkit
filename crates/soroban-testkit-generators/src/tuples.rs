//! Composite strategies for the argument shapes Soroban contracts take most
//! often: small tuples like `(Address, i128)` and fixed-size byte arrays like
//! a `[u8; 32]` hash or a `[u8; 64]` ed25519 signature.
//!
//! Every strategy here is a thin, named wrapper over proptest's own tuple and
//! array strategies, so shrinking is proptest's: a failing tuple shrinks one
//! component at a time while the others stay fixed, and a failing byte array
//! shrinks each byte towards zero independently.

use proptest::array::{uniform, UniformArrayStrategy};
use proptest::num::u8::Any as AnyU8;
use proptest::prelude::*;

/// Generates a pair, one component from each strategy.
///
/// The value is a plain Rust tuple, so once each component is a host type —
/// `(Address, i128)`, `(Symbol, u32)` — it converts with
/// [`soroban_sdk::IntoVal`] into the `Val` a contract argument expects.
///
/// Shrinking simplifies each component independently: proptest tries to
/// simplify the first, then the second, keeping whichever reductions still
/// fail.
pub fn soroban_tuple_2<A, B>(first: A, second: B) -> impl Strategy<Value = (A::Value, B::Value)>
where
    A: Strategy,
    B: Strategy,
{
    (first, second)
}

/// Generates a triple, one component from each strategy.
///
/// See [`soroban_tuple_2`]; the same conversion and shrinking rules apply to
/// each of the three components.
pub fn soroban_tuple_3<A, B, C>(
    first: A,
    second: B,
    third: C,
) -> impl Strategy<Value = (A::Value, B::Value, C::Value)>
where
    A: Strategy,
    B: Strategy,
    C: Strategy,
{
    (first, second, third)
}

/// A fixed-size array of uniformly random bytes.
pub type ByteArrayStrategy<const N: usize> = UniformArrayStrategy<AnyU8, [u8; N]>;

/// Generates `N` random bytes.
///
/// Every byte shrinks towards `0x00` on its own, so a failure that only
/// depends on one byte minimises to an array that is zero everywhere else.
pub fn byte_array<const N: usize>() -> ByteArrayStrategy<N> {
    uniform::<_, N>(any::<u8>())
}

/// Generates a random `[u8; 32]`, the shape of a SHA-256 hash, a contract id,
/// an ed25519 public key or a seed.
///
/// Turn it into the host type with `BytesN::<32>::from_array(&env, &bytes)`.
pub fn byte_array_32() -> ByteArrayStrategy<32> {
    byte_array::<32>()
}

/// Generates a random `[u8; 64]`, the shape of an ed25519 signature.
///
/// Turn it into the host type with `BytesN::<64>::from_array(&env, &bytes)`.
pub fn byte_array_64() -> ByteArrayStrategy<64> {
    byte_array::<64>()
}

#[cfg(test)]
mod tests {
    use super::{byte_array, byte_array_32, byte_array_64, soroban_tuple_2, soroban_tuple_3};
    use crate::strategies::{account_address, token_amount};
    use proptest::prelude::*;
    use proptest::strategy::ValueTree;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::{
        contract, contractimpl, Address, BytesN, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
    };

    #[contract]
    pub struct Allowances;

    #[contractimpl]
    impl Allowances {
        /// Echoes a `(owner, spender, amount)` tuple back, so the test proves the
        /// host decoded exactly the value the strategy produced.
        pub fn echo(_env: Env, grant: (Address, Address, i128)) -> (Address, Address, i128) {
            grant
        }

        pub fn digest_len(_env: Env, digest: BytesN<32>, signature: BytesN<64>) -> u32 {
            digest.len() + signature.len()
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn a_pair_converts_into_a_host_tuple(
            (owner, amount) in soroban_tuple_2(account_address(), token_amount()),
        ) {
            let env = Env::default();
            let pair = (owner.to_address(&env), amount);

            let val: Val = pair.clone().into_val(&env);
            let back = <(Address, i128)>::try_from_val(&env, &val).unwrap();

            prop_assert_eq!(back, pair);
        }

        #[test]
        fn a_triple_survives_a_round_trip_through_a_contract(
            (owner, spender, amount) in soroban_tuple_3(
                account_address(),
                account_address(),
                token_amount(),
            ),
        ) {
            let env = Env::default();
            let client = AllowancesClient::new(&env, &env.register(Allowances, ()));
            let grant = (owner.to_address(&env), spender.to_address(&env), amount);

            prop_assert_eq!(client.echo(&grant), grant);
        }

        #[test]
        fn byte_arrays_become_bytes_n(
            digest in byte_array_32(),
            signature in byte_array_64(),
        ) {
            let env = Env::default();
            let client = AllowancesClient::new(&env, &env.register(Allowances, ()));
            let digest_n = BytesN::from_array(&env, &digest);
            let signature_n = BytesN::from_array(&env, &signature);

            prop_assert_eq!(digest_n.to_array(), digest);
            prop_assert_eq!(signature_n.to_array(), signature);
            prop_assert_eq!(client.digest_len(&digest_n, &signature_n), 96);
        }

        #[test]
        fn tuple_components_can_be_any_host_value(
            (name, count) in soroban_tuple_2("[a-z]{1,10}", any::<u32>()),
        ) {
            let env = Env::default();
            let pair = (Symbol::new(&env, &name), count);
            let as_vec: Vec<Val> = Vec::from_array(&env, [pair.0.to_val(), pair.1.into_val(&env)]);

            prop_assert_eq!(as_vec.len(), 2);
        }
    }

    #[test]
    fn arrays_have_the_advertised_length() {
        let mut runner = TestRunner::deterministic();
        let short = byte_array::<4>().new_tree(&mut runner).unwrap().current();
        let digest = byte_array_32().new_tree(&mut runner).unwrap().current();
        let signature = byte_array_64().new_tree(&mut runner).unwrap().current();

        assert_eq!(short.len(), 4);
        assert_eq!(digest.len(), 32);
        assert_eq!(signature.len(), 64);
    }

    #[test]
    fn arrays_are_not_constant() {
        let mut runner = TestRunner::deterministic();
        let draws: std::collections::HashSet<[u8; 32]> = (0..32)
            .map(|_| byte_array_32().new_tree(&mut runner).unwrap().current())
            .collect();

        assert!(draws.len() > 30, "only {} distinct arrays", draws.len());
    }

    /// Drive proptest's shrinker by hand: keep simplifying while `fails` holds.
    fn minimise<T: Strategy>(strategy: T, fails: impl Fn(&T::Value) -> bool) -> T::Value
    where
        T::Value: Clone,
    {
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
    fn a_byte_array_shrinks_to_zero_outside_the_byte_that_matters() {
        // Fails whenever the first byte is at least 0x10.
        let smallest = minimise(byte_array_32(), |bytes| bytes[0] >= 0x10);

        assert_eq!(smallest[0], 0x10);
        assert!(smallest[1..].iter().all(|&byte| byte == 0), "{smallest:?}");
    }

    #[test]
    fn a_signature_shrinks_every_byte_independently() {
        let smallest = minimise(byte_array_64(), |bytes| bytes[63] >= 7);

        assert_eq!(smallest[63], 7);
        assert!(smallest[..63].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_tuple_shrinks_each_component_on_its_own() {
        // Only the amount matters; the count should shrink to zero regardless.
        let smallest = minimise(
            soroban_tuple_2(0u32..1_000, 0i128..1_000_000),
            |(_, amount)| *amount >= 500,
        );

        assert_eq!(smallest, (0, 500));
    }

    #[test]
    fn a_triple_shrinks_towards_the_simplest_failing_case() {
        let smallest = minimise(
            soroban_tuple_3(0u32..100, 0u32..100, 0u32..100),
            |(a, b, c)| a + b + c >= 50,
        );

        assert_eq!(smallest.0 + smallest.1 + smallest.2, 50);
    }
}
