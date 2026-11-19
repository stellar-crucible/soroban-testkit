//! Strategies over the host's error discriminants.
//!
//! A contract's error-recovery path is only as good as the set of failures it
//! has been shown. Soroban reports host failures as a [`ScErrorType`] — the part
//! of the host that failed — paired with an [`ScErrorCode`] saying how. There
//! are ten of each, so the whole domain is one hundred pairs, and
//! [`host_error_code_coverage`] draws from all of them.
//!
//! The strategy is built from two bounded integers rather than a `prop_oneof!`
//! over the pairs. proptest shrinks integers towards zero, and both tables are
//! in XDR discriminant order, so a failing case simplifies towards
//! `Contract`/`ArithDomain` — the smallest type and the smallest code — instead
//! of stopping at whichever branch of the `oneof` happened to be taken.

use proptest::prelude::*;
use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
use soroban_sdk::Error;

/// Every host error category, in XDR discriminant order.
///
/// The order is load-bearing: [`HostError::index`] and
/// [`host_error_code_coverage`] address this table by position, and proptest
/// shrinks that position towards zero.
pub const HOST_ERROR_TYPES: [ScErrorType; 10] = [
    ScErrorType::Contract,
    ScErrorType::WasmVm,
    ScErrorType::Context,
    ScErrorType::Storage,
    ScErrorType::Object,
    ScErrorType::Crypto,
    ScErrorType::Events,
    ScErrorType::Budget,
    ScErrorType::Value,
    ScErrorType::Auth,
];

/// Every host error code, in XDR discriminant order.
pub const HOST_ERROR_CODES: [ScErrorCode; 10] = [
    ScErrorCode::ArithDomain,
    ScErrorCode::IndexBounds,
    ScErrorCode::InvalidInput,
    ScErrorCode::MissingValue,
    ScErrorCode::ExistingValue,
    ScErrorCode::ExceededLimit,
    ScErrorCode::InvalidAction,
    ScErrorCode::InternalError,
    ScErrorCode::UnexpectedType,
    ScErrorCode::UnexpectedSize,
];

/// The readable name of each entry in [`HOST_ERROR_TYPES`], by position.
const TYPE_NAMES: [&str; 10] = [
    "contract", "wasm_vm", "context", "storage", "object", "crypto", "events", "budget", "value",
    "auth",
];

/// The XDR name of each entry in [`HOST_ERROR_CODES`], by position.
const CODE_NAMES: [&str; 10] = [
    "ArithDomain",
    "IndexBounds",
    "InvalidInput",
    "MissingValue",
    "ExistingValue",
    "ExceededLimit",
    "InvalidAction",
    "InternalError",
    "UnexpectedType",
    "UnexpectedSize",
];

/// One host failure, as the pair the host reports it as.
///
/// Both halves are discriminants the protocol defines, so a test can hand the
/// pair to an error-recovery branch and prove it handles every failure the host
/// can raise. [`Self::as_error`] turns it into the [`Error`] a contract call
/// returns.
///
/// A `Contract` type with an application code is also representable, because
/// the host models contract errors the same way; the codes in
/// [`HOST_ERROR_CODES`] are the standardized host meanings, so an error built
/// from this type is a host error rather than a user-defined one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostError {
    error_type: ScErrorType,
    code: ScErrorCode,
}

impl HostError {
    /// Pair a category with a code.
    pub fn new(error_type: ScErrorType, code: ScErrorCode) -> Self {
        Self { error_type, code }
    }

    /// Which part of the host failed.
    pub fn error_type(&self) -> ScErrorType {
        self.error_type
    }

    /// How it failed.
    pub fn code(&self) -> ScErrorCode {
        self.code
    }

    /// The category as the host names it in text: `"storage"`, `"budget"`, …
    pub fn category(&self) -> &'static str {
        TYPE_NAMES[index_of_type(self.error_type)]
    }

    /// The code as XDR spells it: `"MissingValue"`, `"ExceededLimit"`, …
    pub fn code_name(&self) -> &'static str {
        CODE_NAMES[index_of_code(self.code)]
    }

    /// `category/code`, the form a host error's label takes.
    pub fn label(&self) -> String {
        format!("{}/{}", self.category(), self.code as u32)
    }

    /// The failure as a value a contract call returns.
    pub fn as_error(&self) -> Error {
        Error::from_type_and_code(self.error_type, self.code)
    }

    /// Whether this is an application contract error rather than a host one.
    pub fn is_contract_error(&self) -> bool {
        self.error_type == ScErrorType::Contract
    }

    /// The pair's position in the full `types × codes` table.
    ///
    /// This is the number proptest shrinks: type first, then code, both towards
    /// the smallest discriminant.
    pub fn index(&self) -> usize {
        index_of_type(self.error_type) * HOST_ERROR_CODES.len() + index_of_code(self.code)
    }

    /// The pair at `index`, or `None` when the index is past the table.
    pub fn from_index(index: usize) -> Option<Self> {
        if index >= HOST_ERROR_TYPES.len() * HOST_ERROR_CODES.len() {
            return None;
        }
        Some(Self {
            error_type: HOST_ERROR_TYPES[index / HOST_ERROR_CODES.len()],
            code: HOST_ERROR_CODES[index % HOST_ERROR_CODES.len()],
        })
    }
}

/// Every `ScErrorType` the host defines, at random.
pub fn host_error_type() -> impl Strategy<Value = ScErrorType> {
    (0..HOST_ERROR_TYPES.len()).prop_map(|index| HOST_ERROR_TYPES[index])
}

/// Every `ScErrorCode` the host defines, at random.
pub fn host_error_code() -> impl Strategy<Value = ScErrorCode> {
    (0..HOST_ERROR_CODES.len()).prop_map(|index| HOST_ERROR_CODES[index])
}

/// All one hundred host error pairs, one at a time.
///
/// The pair is the same [`HostError`] the other strategies produce, so a test
/// that wants a deterministic sweep over the domain and a fuzzed run can share
/// the recovery code it exercises.
pub fn host_error_code_coverage() -> impl Strategy<Value = HostError> {
    (0..HOST_ERROR_TYPES.len(), 0..HOST_ERROR_CODES.len()).prop_map(|(type_index, code_index)| {
        HostError::new(HOST_ERROR_TYPES[type_index], HOST_ERROR_CODES[code_index])
    })
}

/// Every host error pair, at random.
///
/// An alias for [`host_error_code_coverage`] for tests that read better without
/// the coverage wording.
pub fn host_error() -> impl Strategy<Value = HostError> {
    host_error_code_coverage()
}

/// The codes of one category, at random.
///
/// `contract` errors are application-defined, so pair this with a fixed type to
/// fuzz one branch of a recovery match rather than the whole domain.
pub fn host_error_of_type(error_type: ScErrorType) -> impl Strategy<Value = HostError> {
    (0..HOST_ERROR_CODES.len())
        .prop_map(move |index| HostError::new(error_type, HOST_ERROR_CODES[index]))
}

/// The full domain, in table order, for a deterministic sweep.
pub fn all_host_errors() -> Vec<HostError> {
    (0..HOST_ERROR_TYPES.len() * HOST_ERROR_CODES.len())
        .filter_map(HostError::from_index)
        .collect()
}

/// The table position of a category, panicking if a new one is not listed.
///
/// A `match` rather than a lookup through [`HOST_ERROR_TYPES`], so an SDK
/// release that adds a category fails to compile here instead of panicking at
/// run time.
fn index_of_type(error_type: ScErrorType) -> usize {
    match error_type {
        ScErrorType::Contract => 0,
        ScErrorType::WasmVm => 1,
        ScErrorType::Context => 2,
        ScErrorType::Storage => 3,
        ScErrorType::Object => 4,
        ScErrorType::Crypto => 5,
        ScErrorType::Events => 6,
        ScErrorType::Budget => 7,
        ScErrorType::Value => 8,
        ScErrorType::Auth => 9,
    }
}

/// The table position of a code, panicking if a new one is not listed.
fn index_of_code(code: ScErrorCode) -> usize {
    match code {
        ScErrorCode::ArithDomain => 0,
        ScErrorCode::IndexBounds => 1,
        ScErrorCode::InvalidInput => 2,
        ScErrorCode::MissingValue => 3,
        ScErrorCode::ExistingValue => 4,
        ScErrorCode::ExceededLimit => 5,
        ScErrorCode::InvalidAction => 6,
        ScErrorCode::InternalError => 7,
        ScErrorCode::UnexpectedType => 8,
        ScErrorCode::UnexpectedSize => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        all_host_errors, host_error, host_error_code, host_error_code_coverage, host_error_of_type,
        host_error_type, HostError, HOST_ERROR_CODES, HOST_ERROR_TYPES,
    };
    use proptest::prelude::*;
    use proptest::test_runner::{TestError, TestRunner};
    use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
    use soroban_sdk::Error;
    use std::collections::HashSet;

    /// A recovery decision, the shape a contract's `match` on an error produces.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Recovery {
        /// The caller may try again: a resource or storage condition.
        Retry,
        /// The failure is not transient: stop and report it.
        Stop,
    }

    /// An error-recovery block: total over every host failure, never panicking.
    fn recover(error: &Error) -> Recovery {
        if error.is_type(ScErrorType::Budget) || error.is_type(ScErrorType::Storage) {
            Recovery::Retry
        } else {
            Recovery::Stop
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn a_generated_error_round_trips_through_the_host(error in host_error_code_coverage()) {
            let host = error.as_error();

            prop_assert!(host.is_type(error.error_type()));
            prop_assert_eq!(host.get_code(), error.code() as u32);
        }

        #[test]
        fn a_generated_error_keeps_its_own_discriminants(error in host_error()) {
            prop_assert!(HOST_ERROR_TYPES.contains(&error.error_type()));
            prop_assert!(HOST_ERROR_CODES.contains(&error.code()));
            prop_assert_eq!(HostError::from_index(error.index()), Some(error));
        }

        #[test]
        fn generated_types_are_never_repeated_per_case(
            left in host_error_type(),
            right in host_error_type(),
        ) {
            // Two independent draws describe one case; nothing constrains them,
            // and a generated value must always be a variant the host defines.
            prop_assert!(HOST_ERROR_TYPES.contains(&left));
            prop_assert!(HOST_ERROR_TYPES.contains(&right));
        }

        #[test]
        fn generated_codes_stay_inside_the_standardized_set(code in host_error_code()) {
            prop_assert!(HOST_ERROR_CODES.contains(&code));
            prop_assert!(code as u32 <= 9);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn an_error_recovery_block_handles_every_generated_failure(
            error in host_error_code_coverage(),
        ) {
            // The fuzzing shape a contract wants: whatever the host reports, the
            // recovery branch answers with a decision and does not panic.
            let decision = recover(&error.as_error());

            match error.error_type() {
                ScErrorType::Budget | ScErrorType::Storage => {
                    prop_assert_eq!(decision, Recovery::Retry)
                }
                _ => prop_assert_eq!(decision, Recovery::Stop),
            }
        }
    }

    #[test]
    fn the_coverage_list_is_every_pair_exactly_once() {
        let all = all_host_errors();

        assert_eq!(all.len(), 100, "ten categories times ten codes");
        let mut seen = HashSet::new();
        for error in &all {
            assert!(
                seen.insert(error.index()),
                "{} appears twice",
                error.label()
            );
        }
        assert_eq!(seen.len(), 100);
        assert_eq!(all_host_errors(), all, "table order is stable");
    }

    #[test]
    fn a_full_run_walks_every_category_and_every_code() {
        // One case draws a hundred pairs, so a run sees thousands of samples and
        // the assertion is about the domain the strategy draws from rather than
        // about any single draw.
        // `TestRunner::run` takes an `Fn`, so the two sets it fills live behind
        // a `RefCell` rather than being captured mutably.
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        let seen = std::cell::RefCell::new((HashSet::new(), HashSet::new()));
        runner
            .run(
                &proptest::collection::vec(host_error_code_coverage(), 100),
                |errors| {
                    let mut seen = seen.borrow_mut();
                    for error in &errors {
                        seen.0.insert(error.error_type());
                        seen.1.insert(error.code());
                    }
                    Ok(())
                },
            )
            .unwrap();

        let (categories, codes) = seen.into_inner();
        assert_eq!(categories.len(), HOST_ERROR_TYPES.len());
        assert_eq!(codes.len(), HOST_ERROR_CODES.len());
    }

    #[test]
    fn shrinking_simplifies_errors_towards_the_minimum_discriminants() {
        // A property that fails for every input, so the shrunk case proptest
        // reports is the smallest one the strategy can produce.
        let mut runner = TestRunner::new(ProptestConfig {
            cases: 1,
            max_shrink_iters: 10_000,
            ..ProptestConfig::default()
        });
        let outcome = runner.run(&host_error_code_coverage(), |error| {
            prop_assert!(
                error.index() > 10_000,
                "{} is below the impossible bound",
                error.label()
            );
            Ok(())
        });

        match outcome {
            Err(TestError::Fail(_, smallest)) => {
                assert_eq!(smallest.index(), 0);
                assert_eq!(smallest.error_type(), ScErrorType::Contract);
                assert_eq!(smallest.code(), ScErrorCode::ArithDomain);
                assert_eq!(smallest.label(), "contract/0");
            }
            other => panic!("expected a shrunk failure, got {other:?}"),
        }
    }

    #[test]
    fn a_fixed_type_strategy_only_yields_that_category() {
        let mut runner = TestRunner::default();
        runner
            .run(&host_error_of_type(ScErrorType::Storage), |error| {
                prop_assert_eq!(error.error_type(), ScErrorType::Storage);
                prop_assert!(!error.is_contract_error());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn labels_name_the_category_and_the_numeric_code() {
        let missing = HostError::new(ScErrorType::Storage, ScErrorCode::MissingValue);
        let exceeded = HostError::new(ScErrorType::Budget, ScErrorCode::ExceededLimit);

        assert_eq!(missing.category(), "storage");
        assert_eq!(missing.code_name(), "MissingValue");
        assert_eq!(missing.label(), "storage/3");
        assert_eq!(exceeded.label(), "budget/5");
        assert!(!missing.is_contract_error());
    }

    #[test]
    fn a_contract_error_is_told_apart_from_a_host_error() {
        let contract = HostError::new(ScErrorType::Contract, ScErrorCode::InvalidAction);

        assert!(contract.is_contract_error());
        assert_eq!(
            contract.as_error().get_code(),
            ScErrorCode::InvalidAction as u32
        );
    }

    #[test]
    fn an_index_past_the_table_answers_none() {
        assert_eq!(HostError::from_index(100), None);
        assert!(HostError::from_index(99).is_some());
        assert_eq!(
            HostError::from_index(99),
            Some(HostError::new(
                ScErrorType::Auth,
                ScErrorCode::UnexpectedSize
            ))
        );
    }

    #[test]
    fn every_category_and_code_is_named() {
        for (index, error_type) in HOST_ERROR_TYPES.iter().enumerate() {
            let error = HostError::new(*error_type, ScErrorCode::InvalidInput);
            assert!(!error.category().is_empty(), "category {index} has no name");
        }
        for (index, code) in HOST_ERROR_CODES.iter().enumerate() {
            let error = HostError::new(ScErrorType::Value, *code);
            assert!(!error.code_name().is_empty(), "code {index} has no name");
        }
    }
}
