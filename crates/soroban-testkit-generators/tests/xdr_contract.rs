//! Drives a contract that accepts nested XDR types with the `xdr` strategies.

use proptest::prelude::*;
use soroban_sdk::{contract, contractimpl, Bytes, Env, Map, String, Symbol, TryFromVal, Val, Vec};
use soroban_testkit_generators::xdr::{
    xdr_bytes, xdr_map, xdr_string, xdr_symbol, xdr_val, xdr_vec, XdrBounds,
};

/// A registry that indexes tagged documents. Every argument is a complex XDR
/// type, which is exactly the surface the strategies are meant to fuzz.
#[contract]
pub struct Catalogue;

#[contractimpl]
impl Catalogue {
    /// Store `tags` under `label` and return how many tag values were indexed.
    pub fn index(env: Env, label: String, tags: Map<Symbol, Vec<Symbol>>, payload: Bytes) -> u32 {
        let mut total = 0u32;
        for (_, values) in tags.iter() {
            total += values.len();
        }
        env.storage().persistent().set(&label, &(tags, payload));
        total
    }

    /// Read back what `index` stored.
    pub fn lookup(env: Env, label: String) -> Option<(Map<Symbol, Vec<Symbol>>, Bytes)> {
        env.storage().persistent().get(&label)
    }

    /// Accept any value at all and echo it.
    pub fn echo(_env: Env, value: Val) -> Val {
        value
    }

    /// Accept a vector of arbitrary values and count them.
    pub fn count(_env: Env, values: Vec<Val>) -> u32 {
        values.len()
    }
}

fn setup() -> (Env, CatalogueClient<'static>) {
    let env = Env::default();
    let id = env.register(Catalogue, ());
    let client = CatalogueClient::new(&env, &id);
    (env, client)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn nested_maps_of_vectors_round_trip_through_storage(seed in any::<u64>()) {
        let (env, client) = setup();
        let strategy = (
            xdr_string(&env, 24),
            xdr_map(&env, xdr_symbol(&env, 10), xdr_vec(&env, xdr_symbol(&env, 10), 4), 5),
            xdr_bytes(&env, 32),
        );
        let mut runner = proptest::test_runner::TestRunner::new_with_rng(
            proptest::test_runner::Config::default(),
            proptest::test_runner::TestRng::from_seed(
                proptest::test_runner::RngAlgorithm::ChaCha,
                &seed.to_le_bytes().repeat(4),
            ),
        );
        use proptest::strategy::ValueTree;
        let (label, tags, payload) = strategy.new_tree(&mut runner).unwrap().current();

        let expected: u32 = tags.iter().map(|(_, values)| values.len()).sum();
        prop_assert_eq!(client.index(&label, &tags, &payload), expected);
        let (stored_tags, stored_payload) = client.lookup(&label).unwrap();
        prop_assert_eq!(stored_tags, tags);
        prop_assert_eq!(stored_payload, payload);
    }

    #[test]
    fn arbitrary_vals_cross_the_contract_boundary(seed in any::<u64>()) {
        let (env, client) = setup();
        let strategy = xdr_val(&env, XdrBounds::default().with_max_depth(3).with_max_collection_len(4));
        let mut runner = proptest::test_runner::TestRunner::new_with_rng(
            proptest::test_runner::Config::default(),
            proptest::test_runner::TestRng::from_seed(
                proptest::test_runner::RngAlgorithm::ChaCha,
                &seed.to_le_bytes().repeat(4),
            ),
        );
        use proptest::strategy::ValueTree;
        let value = strategy.new_tree(&mut runner).unwrap().current();
        let echoed = client.echo(&value);
        let original = soroban_sdk::xdr::ScVal::try_from_val(&env, &value).unwrap();
        let returned = soroban_sdk::xdr::ScVal::try_from_val(&env, &echoed).unwrap();
        prop_assert_eq!(original, returned);
    }
}

#[test]
fn vectors_of_arbitrary_values_are_counted() {
    use proptest::strategy::ValueTree;
    let (env, client) = setup();
    let strategy = xdr_vec(
        &env,
        xdr_val(&env, XdrBounds::default().with_max_depth(1)),
        6,
    );
    let mut runner = proptest::test_runner::TestRunner::default();
    for _ in 0..32 {
        let values = strategy.new_tree(&mut runner).unwrap().current();
        assert_eq!(client.count(&values), values.len());
    }
}
