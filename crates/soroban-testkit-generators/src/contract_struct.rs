//! Generate proptest strategies for user-defined `#[contracttype]` structs.
//!
//! Soroban contracts frequently model state with nested `#[contracttype]`
//! structs: a position that owns a configuration, a batch that holds a vector
//! of child records, or an order with an optional fill. Hand-rolling a
//! `Strategy` for every one of them is repetitive, and a hand-rolled tuple
//! mapping is easy to get subtly wrong.
//!
//! [`prop_contract_struct!`](crate::prop_contract_struct) takes the struct
//! path and one strategy per field and synthesises a function that returns an
//! `impl Strategy<Value = TheStruct>`:
//!
//! ```rust
//! use proptest::prelude::*;
//! use soroban_sdk::contracttype;
//! use soroban_testkit_generators::{prop_contract_struct, strategies};
//!
//! #[contracttype]
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub struct Limit {
//!     pub amount: i128,
//!     pub enabled: bool,
//! }
//!
//! #[contracttype]
//! #[derive(Clone, Debug, PartialEq, Eq)]
//! pub struct Order {
//!     pub limit: Limit,
//!     pub backup: Option<i128>,
//!     pub tags: soroban_sdk::Vec<u32>,
//!     pub count: u32,
//! }
//!
//! prop_contract_struct! {
//!     pub fn limit_strategy() -> Limit {
//!         amount: strategies::token_amount(),
//!         enabled: any::<bool>(),
//!     }
//! }
//!
//! prop_contract_struct! {
//!     pub fn order_strategy(env: soroban_sdk::Env) -> Order {
//!         limit: limit_strategy(),
//!         backup: prop::option::of(strategies::token_amount()),
//!         tags: strategies::soroban_vec(0u32..=100, 4, env),
//!         count: 0u32..=100,
//!     }
//! }
//! ```
//!
//! # Nested values, `Option`, enums, and collections
//!
//! The macro composes: a child struct is just another strategy expression, so
//! `limit_strategy()` above could equally be wrapped in
//! `prop::collection::vec` or `prop::collection::hash_map`.
//! Contract enums work the same way — pass a strategy built with `prop_oneof!`
//! for the enum field, and the macro handles the surrounding struct.
//!
//! Shrinking is recursive because the generated strategy is a proptest tuple:
//! each field keeps its own shrink tree, so a failing nested order shrinks the
//! optional limit, the vector contents, and the scalar fields independently.
//!
//! The generated `prop_map` uses a proptest tuple, which supports up to 12
//! fields per struct. Nest wider structs by grouping related fields into a
//! child `#[contracttype]` struct.

/// Synthesise a proptest strategy for a `#[contracttype]` struct.
///
/// The invocation names the generated function, its parameters, the struct
/// name, and one strategy expression per field:
///
/// ```rust,ignore
/// prop_contract_struct! {
///     /// Strategy for `Position`.
///     pub fn position_strategy(env: Env) -> Position {
///         owner: strategies::account_address(),
///         amount: strategies::token_amount(),
///         active: any::<bool>(),
///         history: strategies::soroban_vec(strategies::token_amount(), 8, env),
///     }
/// }
/// ```
///
/// Parameters are forwarded to the generated function, so a strategy can
/// borrow the test `Env` for SDK collections. Attributes and visibility on the
/// generated function are preserved. Field names must match the struct's
/// fields; the struct itself must be in scope at the macro invocation and
/// constructible from those fields.
#[macro_export]
macro_rules! prop_contract_struct {
    (
        $(#[$attr:meta])*
        $vis:vis fn $name:ident($($arg:ident : $arg_ty:ty),* $(,)?) -> $ty:ident {
            $($field:ident : $strategy:expr),* $(,)?
        }
    ) => {
        $(#[$attr])*
        $vis fn $name($($arg: $arg_ty),*) -> impl ::proptest::strategy::Strategy<Value = $ty> {
            use ::proptest::strategy::Strategy as _;

            ($($strategy,)*).prop_map(|($($field,)*)| $ty {
                $($field,)*
            })
        }
    };
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::strategy::ValueTree as _;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::{contracttype, Env, IntoVal as _, TryFromVal as _, Val};

    use crate::strategies::{soroban_vec, token_amount};

    #[contracttype]
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Level {
        pub value: u32,
        pub flag: bool,
    }

    #[contracttype]
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Mode {
        Idle,
        Active(u32),
    }

    #[contracttype]
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct Order {
        pub level: Level,
        pub limit: Option<i128>,
        pub tags: soroban_sdk::Vec<u32>,
        pub mode: Mode,
    }

    prop_contract_struct! {
        fn level_strategy() -> Level {
            value: 0u32..=1_000,
            flag: any::<bool>(),
        }
    }

    fn mode_strategy() -> impl Strategy<Value = Mode> {
        prop_oneof![Just(Mode::Idle), (0u32..=1_000).prop_map(Mode::Active),]
    }

    prop_contract_struct! {
        fn order_strategy(env: Env) -> Order {
            level: level_strategy(),
            limit: prop::option::of(token_amount()),
            tags: soroban_vec(0u32..=100, 4, env),
            mode: mode_strategy(),
        }
    }

    #[test]
    fn generated_strategy_builds_nested_structs() {
        let env = Env::default();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        runner
            .run(&order_strategy(env.clone()), |order| {
                prop_assert!(order.level.value <= 1_000);
                prop_assert!(order.tags.len() <= 4);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn generated_structs_round_trip_through_contract_types() {
        let env = Env::default();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        runner
            .run(&order_strategy(env.clone()), |order| {
                let val: Val = order.clone().into_val(&env);
                let decoded = Order::try_from_val(&env, &val).unwrap();
                prop_assert_eq!(decoded, order);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn nested_members_shrink_recursively() {
        let env = Env::default();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(1));
        let mut tree = order_strategy(env).new_tree(&mut runner).unwrap();
        while tree.simplify() {}

        let order = tree.current();
        assert_eq!(
            order.level,
            Level {
                value: 0,
                flag: false,
            }
        );
        assert_eq!(order.limit, None);
        assert!(order.tags.is_empty());
        assert_eq!(order.mode, Mode::Idle);
    }
}
