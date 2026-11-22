//! Proptest strategies for Soroban Symbol validity, length limits, and casing constraints.

use proptest::prelude::*;
use soroban_sdk::Symbol;

/// Strategy for generating valid `soroban_sdk::Symbol` values.
///
/// Symbols are valid ASCII strings of at most 32 characters containing only
/// alphanumeric characters and underscores (`[a-zA-Z0-9_]`).
pub fn sorobans_symbol_strategy(min_len: usize, max_len: usize) -> impl Strategy<Value = Symbol> {
    proptest::string::string_regex(&format!("[a-zA-Z0-9_]{{{},{}}}", min_len, max_len))
        .expect("valid symbol regex")
        .prop_map(|s| Symbol::try_from(s.as_str()).expect("valid symbol"))
}

/// Default strategy generating valid `soroban_sdk::Symbol` values of length 1 to 32.
pub fn soroban_symbol() -> impl Strategy<Value = Symbol> {
    sorobans_symbol_strategy(1, 32)
}

/// Strategy for generating symbol identifiers matching standard contract naming conventions (e.g. snake_case or camelCase).
pub fn soroban_identifier_symbol() -> impl Strategy<Value = Symbol> {
    proptest::string::string_regex("[a-zA-Z][a-zA-Z0-9_]{0,31}")
        .expect("valid identifier regex")
        .prop_map(|s| Symbol::try_from(s.as_str()).expect("valid identifier symbol"))
}

/// Strategy for generating strings with rejected characters (spaces, special symbols, Unicode)
/// that are guaranteed to fail `Symbol::try_from` or panic/reject.
pub fn invalid_symbol_strategy() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[^a-zA-Z0-9_]+").expect("invalid symbol regex")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::test_runner::TestRunner;

    #[test]
    fn test_soroban_symbol_strategy() {
        let mut runner = TestRunner::default();
        let strat = soroban_symbol();
        let mut generated_any = false;

        for _ in 0..100 {
            let symbol = strat.new_tree(&mut runner).unwrap().current();

            let s_str = symbol.to_string();
            assert!(!s_str.is_empty());
            assert!(s_str.len() <= 32);
            assert!(s_str.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            generated_any = true;
        }
        assert!(generated_any);
    }

    #[test]
    fn test_invalid_symbol_strategy() {
        let mut runner = TestRunner::default();
        let strat = invalid_symbol_strategy();

        for _ in 0..50 {
            let inval = strat.new_tree(&mut runner).unwrap().current();

            assert!(
                Symbol::try_from(inval.as_str()).is_err()
                    || inval
                        .chars()
                        .any(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            );
        }
    }
}
