//! Differential testing of a contract against a pure-Rust reference model.
//!
//! A unit test says what a contract returns for one input. A differential test
//! says that for *every* input the contract and an independent model of the same
//! rule agree — which is the only way to catch the class of bug where both the
//! contract and its tests encode the same wrong assumption, because the model is
//! written from the spec rather than from the implementation.
//!
//! [`DifferentialHarness`] feeds one generated input to both sides, compares the
//! return value and, optionally, the storage state each leaves behind, and hands
//! the whole comparison to `proptest` so that a divergence is shrunk to the
//! smallest input that still reproduces it.
//!
//! ```
//! use soroban_testkit_generators::differential::DifferentialHarness;
//! use soroban_testkit_generators::strategies::token_amount;
//!
//! // The contract rounds a remainder up; the model truncates. They disagree by
//! // exactly one unit for odd amounts.
//! let contract_fee = |amount: i128| amount / 2 + amount % 2;
//! let model_fee = |amount: i128| amount / 2;
//!
//! let report = DifferentialHarness::new().diff(token_amount(), contract_fee, model_fee);
//! let failure = report.failure().expect("the two rules disagree");
//! assert_eq!(failure.divergence().delta(), Some(1));
//!
//! // ...and a one-unit tolerance is what says that disagreement is acceptable.
//! let tolerated = DifferentialHarness::new()
//!     .tolerance(1)
//!     .diff(token_amount(), contract_fee, model_fee);
//! assert!(tolerated.is_match());
//! ```
//!
//! Compare against a deployed contract by passing a client call as the first
//! closure and a `fn` as the model:
//!
//! ```no_run
//! # use soroban_testkit_generators::differential::DifferentialHarness;
//! # use soroban_testkit_generators::strategies::token_amount;
//! # struct Client;
//! # impl Client { fn calculate_fee(&self, amount: &i128) -> i128 { *amount } }
//! # let client = Client;
//! # fn reference_fee_calculator(amount: i128) -> i128 { amount }
//! let harness = DifferentialHarness::new();
//! harness.compare(
//!     token_amount(),
//!     |amount| client.calculate_fee(&amount),
//!     |amount| reference_fee_calculator(amount),
//! );
//! ```
//!
//! [`DifferentialHarness::compare`] asserts. Use
//! [`DifferentialHarness::diff`] when the divergence is data you want to keep —
//! to assert on its shape, or to write into a report.

use proptest::prelude::*;
use proptest::test_runner::{Config, Reason, TestCaseError, TestError, TestRunner};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// One generated input has made the contract and its model disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    kind: DivergenceKind,
    contract: DiffValue,
    reference: DiffValue,
    delta: Option<i128>,
}

impl Divergence {
    /// Which part of the execution disagreed.
    pub fn kind(&self) -> &DivergenceKind {
        &self.kind
    }

    /// Where the disagreement is, as a phrase: `return value` or a key name.
    pub fn location(&self) -> String {
        match &self.kind {
            DivergenceKind::Return => "return value".to_string(),
            DivergenceKind::State { key } => format!("storage key `{key}`"),
        }
    }

    /// What the contract produced.
    pub fn contract_value(&self) -> &DiffValue {
        &self.contract
    }

    /// What the reference model produced.
    pub fn reference_value(&self) -> &DiffValue {
        &self.reference
    }

    /// How far apart the two sides are, for numeric values. `None` when the
    /// values are not both numbers, since a boolean and a `u32` of `1` are not
    /// one unit apart — they are different types.
    pub fn delta(&self) -> Option<i128> {
        self.delta
    }

    /// How the values differ, in a line.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "{}: contract {}, model {}",
            self.location(),
            self.contract,
            self.reference
        );
        if let Some(delta) = self.delta {
            line.push_str(&format!(" (contract is {delta} from the model)"));
        }
        line
    }
}

/// Which part of an execution a [`Divergence`] is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DivergenceKind {
    /// The two sides returned different values.
    Return,
    /// The two sides left a storage key holding different values, or holding it
    /// at all.
    State {
        /// The key, as named by whichever side recorded it.
        key: String,
    },
}

/// A value from either side of a comparison, reduced to something comparable.
///
/// The variants cover what a Soroban contract can return and store: integers in
/// both signedness, booleans, bytes, text and lists of any of them. Anything the
/// harness cannot reduce is your problem to reduce first — a struct becomes its
/// fields, a map becomes sorted pairs — because a comparison that quietly treats
/// two unequal things as equal is worse than no comparison at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffValue {
    /// A signed integer, the `i128` a contract call returns.
    Signed(i128),
    /// An unsigned integer, the `u32` or `u128` a contract call returns.
    Unsigned(u128),
    /// A boolean.
    Bool(bool),
    /// Raw bytes, the `Bytes` and `BytesN` types.
    Bytes(Vec<u8>),
    /// Text.
    Text(String),
    /// An ordered sequence of values.
    List(Vec<DiffValue>),
    /// A call that returns nothing, or a stored `()` value.
    None,
    /// A storage key one side wrote and the other did not write at all.
    Unset,
}

impl DiffValue {
    /// A list, from anything that yields values.
    pub fn list(values: impl IntoIterator<Item = DiffValue>) -> Self {
        DiffValue::List(values.into_iter().collect())
    }

    /// The value as a signed integer, if it is one that fits.
    ///
    /// A `u128` above `i128::MAX` has no signed form, which the tolerance rule
    /// treats as "not comparable" rather than wrapping it.
    pub fn as_i128(&self) -> Option<i128> {
        match self {
            DiffValue::Signed(value) => Some(*value),
            DiffValue::Unsigned(value) => i128::try_from(*value).ok(),
            _ => None,
        }
    }
}

impl fmt::Display for DiffValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiffValue::Signed(value) => write!(f, "{value}"),
            DiffValue::Unsigned(value) => write!(f, "{value}"),
            DiffValue::Bool(value) => write!(f, "{value}"),
            DiffValue::Bytes(value) => {
                write!(f, "bytes[{}]<", value.len())?;
                for byte in value.iter().take(8) {
                    write!(f, "{byte:02x}")?;
                }
                if value.len() > 8 {
                    write!(f, "…")?;
                }
                write!(f, ">")
            }
            DiffValue::Text(value) => write!(f, "{value:?}"),
            DiffValue::List(values) => {
                let rendered: Vec<String> = values.iter().map(ToString::to_string).collect();
                write!(f, "[{}]", rendered.join(", "))
            }
            DiffValue::None => write!(f, "none"),
            DiffValue::Unset => write!(f, "<unset>"),
        }
    }
}

macro_rules! diff_value_from_int {
    ($($type:ty => $variant:ident),* $(,)?) => {
        $(
            impl From<$type> for DiffValue {
                fn from(value: $type) -> Self {
                    DiffValue::$variant(value as _)
                }
            }
        )*
    };
}

diff_value_from_int!(
    i8 => Signed,
    i16 => Signed,
    i32 => Signed,
    i64 => Signed,
    i128 => Signed,
    u8 => Unsigned,
    u16 => Unsigned,
    u32 => Unsigned,
    u64 => Unsigned,
    u128 => Unsigned,
);

impl From<bool> for DiffValue {
    fn from(value: bool) -> Self {
        DiffValue::Bool(value)
    }
}

impl From<Vec<u8>> for DiffValue {
    fn from(value: Vec<u8>) -> Self {
        DiffValue::Bytes(value)
    }
}

impl From<&[u8]> for DiffValue {
    fn from(value: &[u8]) -> Self {
        DiffValue::Bytes(value.to_vec())
    }
}

impl From<String> for DiffValue {
    fn from(value: String) -> Self {
        DiffValue::Text(value)
    }
}

impl From<&str> for DiffValue {
    fn from(value: &str) -> Self {
        DiffValue::Text(value.to_string())
    }
}

impl From<Vec<DiffValue>> for DiffValue {
    fn from(value: Vec<DiffValue>) -> Self {
        DiffValue::List(value)
    }
}

impl<T: Into<DiffValue>> From<Option<T>> for DiffValue {
    fn from(value: Option<T>) -> Self {
        value.map_or(DiffValue::None, Into::into)
    }
}

impl From<()> for DiffValue {
    fn from(_: ()) -> Self {
        DiffValue::None
    }
}

/// What one side of a comparison produced: its return value and, for the
/// contract side, the storage it leaves behind.
///
/// Return values alone miss the bugs that matter most on a ledger, where a call
/// can return the right number and write the wrong state. Record the state the
/// spec says the call should leave and the harness compares both.
///
/// ```
/// use soroban_testkit_generators::differential::{DiffValue, ExecutionTrace};
///
/// let trace = ExecutionTrace::new(1_500i128).with_state("fee_total", 1_500i128);
/// assert_eq!(trace.result(), &DiffValue::Signed(1_500));
/// assert_eq!(trace.state()["fee_total"], DiffValue::Signed(1_500));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionTrace {
    result: DiffValue,
    state: BTreeMap<String, DiffValue>,
}

impl ExecutionTrace {
    /// A trace of a return value that writes nothing you care about.
    pub fn new(result: impl Into<DiffValue>) -> Self {
        Self {
            result: result.into(),
            state: BTreeMap::new(),
        }
    }

    /// Add a storage key this call left behind.
    ///
    /// Keys are named by you, not read off the ledger, so that the model side can
    /// name the same key without building an `Env`.
    pub fn with_state(mut self, key: impl Into<String>, value: impl Into<DiffValue>) -> Self {
        self.state.insert(key.into(), value.into());
        self
    }

    /// The return value.
    pub fn result(&self) -> &DiffValue {
        &self.result
    }

    /// The storage keys, in key order.
    pub fn state(&self) -> &BTreeMap<String, DiffValue> {
        &self.state
    }
}

/// What a side of a comparison may return.
///
/// Implemented for [`ExecutionTrace`] and for every type [`DiffValue`] is built
/// from, so a closure returning `i128` and a closure returning
/// [`ExecutionTrace`] both work — the first just compares return values.
pub trait IntoTrace {
    /// Reduce this side's output to a trace.
    fn into_trace(self) -> ExecutionTrace;
}

impl IntoTrace for ExecutionTrace {
    fn into_trace(self) -> ExecutionTrace {
        self
    }
}

impl IntoTrace for DiffValue {
    fn into_trace(self) -> ExecutionTrace {
        ExecutionTrace::new(self)
    }
}

macro_rules! trace_from_value {
    ($($type:ty),* $(,)?) => {
        $(
            impl IntoTrace for $type {
                fn into_trace(self) -> ExecutionTrace {
                    ExecutionTrace::new(self)
                }
            }
        )*
    };
}

trace_from_value!(
    bool,
    i8,
    i16,
    i32,
    i64,
    i128,
    u8,
    u16,
    u32,
    u64,
    u128,
    String,
    &str,
    &[u8],
    Vec<u8>,
    Vec<DiffValue>,
    (),
    Option<i128>,
    Option<u128>,
    Option<u32>,
    Option<bool>,
    Option<Vec<u8>>,
    Option<String>,
);

/// One comparison: how many cases ran, and the smallest input that diverged.
#[derive(Clone, Debug)]
pub struct DifferentialReport {
    cases: u32,
    tolerance: i128,
    failure: Option<DifferentialFailure>,
}

impl DifferentialReport {
    /// The case budget the comparison ran under.
    pub fn cases(&self) -> u32 {
        self.cases
    }

    /// The numeric tolerance the comparison was given.
    pub fn tolerance(&self) -> i128 {
        self.tolerance
    }

    /// Whether every case agreed.
    pub fn is_match(&self) -> bool {
        self.failure.is_none()
    }

    /// The divergence found, if any.
    pub fn failure(&self) -> Option<&DifferentialFailure> {
        self.failure.as_ref()
    }

    /// Panic with the failing payload, if the two sides disagreed.
    ///
    /// Panicking is what makes the harness usable as an assertion inside an
    /// ordinary `#[test]`; [`DifferentialHarness::compare`] is this method on the
    /// other side of a run.
    pub fn assert_matches(&self) {
        if let Some(failure) = &self.failure {
            panic!("{}", failure.render(self.tolerance));
        }
    }
}

/// The smallest input that still makes the contract and its model disagree.
#[derive(Clone, Debug)]
pub struct DifferentialFailure {
    input: String,
    divergence: Divergence,
}

impl DifferentialFailure {
    /// The shrunk payload, rendered the way `Debug` renders it. This is the input
    /// to paste back into a regression test.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// What the two sides did with it.
    pub fn divergence(&self) -> &Divergence {
        &self.divergence
    }

    /// The failure as a block of text, for a panic message or a CI log.
    pub fn render(&self, tolerance: i128) -> String {
        format!(
            "contract diverged from its reference model\n\
             \tsmallest failing input: {input}\n\
             \t{description}\n\
             \ttolerance: {tolerance}",
            input = self.input,
            description = self.divergence.describe(),
        )
    }
}

/// Runs a contract and a reference model over the same generated inputs.
///
/// The default is exact equality over 256 cases. [`tolerance`](Self::tolerance)
/// allows a fixed-point drift of up to that many units, and
/// [`cases`](Self::cases) trades coverage for wall-clock time — which matters
/// when the contract side deploys an `Env`.
#[derive(Clone, Debug)]
pub struct DifferentialHarness {
    cases: u32,
    tolerance: i128,
    seed: Option<u64>,
}

impl Default for DifferentialHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl DifferentialHarness {
    /// Cases run when [`cases`](Self::cases) is not called: `proptest`'s own
    /// default, so a differential test costs what a property test costs.
    pub const DEFAULT_CASES: u32 = 256;

    /// A harness requiring exact agreement over [`DEFAULT_CASES`](Self::DEFAULT_CASES).
    pub fn new() -> Self {
        Self {
            cases: Self::DEFAULT_CASES,
            tolerance: 0,
            seed: None,
        }
    }

    /// How many generated inputs to compare.
    pub fn cases(mut self, cases: u32) -> Self {
        self.cases = cases;
        self
    }

    /// Accept numeric results up to `tolerance` apart.
    ///
    /// For a fee taken in basis points and rounded once, a model that rounds in
    /// the other direction is one unit out — `.tolerance(1)` says so. Non-numeric
    /// results ignore the tolerance: a `false` is never within one unit of `true`.
    pub fn tolerance(mut self, tolerance: i128) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Generate from a fixed seed, so a divergence reproduces input for input.
    ///
    /// A shrunk payload already identifies the failing case, but a seed also
    /// fixes the order the rest of the inputs arrived in, which is what you want
    /// while bisecting a flaky-looking model.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Compare both sides and assert they never diverge.
    ///
    /// Panics with the smallest failing input and what each side did with it.
    pub fn compare<S, C, R, CO, RO>(&self, strategy: S, contract: C, reference: R)
    where
        S: Strategy,
        S::Value: Clone + fmt::Debug,
        C: Fn(S::Value) -> CO,
        R: Fn(S::Value) -> RO,
        CO: IntoTrace,
        RO: IntoTrace,
    {
        self.diff(strategy, contract, reference).assert_matches();
    }

    /// Compare both sides and hand back what was found.
    ///
    /// The first closure is the contract, the second the model. Both receive the
    /// generated input by value, so a contract call borrows it
    /// (`|amount| client.fee(&amount)`) while a model consumes it
    /// (`|amount| fee(amount)`).
    ///
    /// Shrinking is `proptest`'s: the payload in the report is the smallest input
    /// in the strategy that still diverges, not the first one that happened to.
    pub fn diff<S, C, R, CO, RO>(
        &self,
        strategy: S,
        contract: C,
        reference: R,
    ) -> DifferentialReport
    where
        S: Strategy,
        S::Value: Clone + fmt::Debug,
        C: Fn(S::Value) -> CO,
        R: Fn(S::Value) -> RO,
        CO: IntoTrace,
        RO: IntoTrace,
    {
        let config = Config {
            cases: self.cases,
            failure_persistence: None,
            rng_seed: self.seed.map_or_else(
                || proptest::test_runner::RngSeed::Random,
                proptest::test_runner::RngSeed::Fixed,
            ),
            ..Config::default()
        };
        let mut runner = TestRunner::new(config);
        // A shrinking run fails on every candidate it accepts, so the divergence
        // recorded last is the one for the smallest failing input.
        let diverged = Cell::new(None);
        let tolerance = self.tolerance;

        let outcome = runner.run(&strategy, |input| {
            let contract_trace = contract(input.clone()).into_trace();
            let reference_trace = reference(input).into_trace();
            match divergence_between(&contract_trace, &reference_trace, tolerance) {
                None => Ok(()),
                Some(divergence) => {
                    diverged.set(Some(divergence));
                    Err(TestCaseError::Fail(Reason::from(
                        "contract diverged from its reference model",
                    )))
                }
            }
        });

        let failure = match outcome {
            Ok(()) => None,
            Err(TestError::Fail(_, input)) => {
                diverged.take().map(|divergence| DifferentialFailure {
                    input: format!("{input:?}"),
                    divergence,
                })
            }
            Err(TestError::Abort(reason)) => panic!(
                "differential comparison aborted after {} cases: {reason}",
                self.cases
            ),
        };

        DifferentialReport {
            cases: self.cases,
            tolerance: self.tolerance,
            failure,
        }
    }
}

/// The first disagreement between two sides, return value before state.
fn divergence_between(
    contract: &ExecutionTrace,
    reference: &ExecutionTrace,
    tolerance: i128,
) -> Option<Divergence> {
    if let Some(mut divergence) = mismatch(&contract.result, &reference.result, tolerance) {
        divergence.kind = DivergenceKind::Return;
        return Some(divergence);
    }

    let keys: BTreeSet<&String> = contract
        .state
        .keys()
        .chain(reference.state.keys())
        .collect();
    for key in keys {
        let mine = contract.state.get(key).cloned().unwrap_or(DiffValue::Unset);
        let theirs = reference
            .state
            .get(key)
            .cloned()
            .unwrap_or(DiffValue::Unset);
        if let Some(mut divergence) = mismatch(&mine, &theirs, tolerance) {
            divergence.kind = DivergenceKind::State { key: key.clone() };
            return Some(divergence);
        }
    }
    None
}

/// Whether two values disagree by more than `tolerance`.
///
/// The placeholder `kind` is filled in by [`divergence_between`], the only side
/// that knows whether this is a return value or a storage key.
fn mismatch(contract: &DiffValue, reference: &DiffValue, tolerance: i128) -> Option<Divergence> {
    if contract == reference {
        return None;
    }
    let delta = match (contract.as_i128(), reference.as_i128()) {
        (Some(mine), Some(theirs)) => mine.checked_sub(theirs),
        _ => None,
    };
    if delta.is_some_and(|delta| delta.unsigned_abs() <= tolerance.unsigned_abs()) {
        return None;
    }
    Some(Divergence {
        kind: DivergenceKind::Return,
        contract: contract.clone(),
        reference: reference.clone(),
        delta,
    })
}

#[cfg(test)]
mod tests {
    use super::{DiffValue, DifferentialHarness, DivergenceKind, ExecutionTrace, IntoTrace};
    use crate::strategies::token_amount;
    use proptest::prelude::*;
    use soroban_sdk::{contract, contractimpl, Env, Symbol};

    /// A fee of half the amount, rounded up, remembered in persistent storage.
    #[contract]
    pub struct Fees;

    #[contractimpl]
    impl Fees {
        pub fn calculate_fee(env: Env, amount: i128) -> i128 {
            let fee = amount / 2 + amount % 2;
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "fee"), &fee);
            fee
        }

        /// Constant-product output with a 0.3% fee taken on the way in.
        pub fn amount_out(env: Env, dx: i128, reserve_x: i128, reserve_y: i128) -> i128 {
            let dx_with_fee = dx * 997;
            let out = (dx_with_fee * reserve_y) / (reserve_x * 1000 + dx_with_fee);
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "last"), &out);
            out
        }
    }

    /// The fee rule as written from its specification, not from the contract.
    fn reference_fee(amount: i128) -> i128 {
        amount / 2 + amount % 2
    }

    fn stored_fee(env: &Env, id: &soroban_sdk::Address) -> Option<i128> {
        let key = Symbol::new(env, "fee");
        env.as_contract(id, || env.storage().persistent().get(&key))
    }

    #[test]
    fn a_contract_and_a_model_that_agree_pass_over_every_amount() {
        let env = Env::default();
        let id = env.register(Fees, ());
        let client = FeesClient::new(&env, &id);

        let report = DifferentialHarness::new().cases(64).diff(
            token_amount(),
            |amount| client.calculate_fee(&amount),
            reference_fee,
        );

        assert!(report.is_match(), "{:?}", report.failure());
        assert_eq!(report.cases(), 64);
        // The run really entered the contract, rather than comparing two closures.
        assert!(stored_fee(&env, &id).is_some(), "the contract wrote no fee");
    }

    #[test]
    fn the_issue_example_compiles_as_a_statement() {
        // `compare` returns unit, so the issue's call stands alone with no `let _
        // =` in front of it. The module doctest checks the same call with the
        // reference spelled as a closure; this one actually runs it.
        let env = Env::default();
        let id = env.register(Fees, ());
        let client = FeesClient::new(&env, &id);
        let harness = DifferentialHarness::new().cases(32);
        harness.compare(
            token_amount(),
            |amount| client.calculate_fee(&amount),
            reference_fee,
        );
    }

    #[test]
    fn a_missing_rounding_term_is_a_divergence_of_one() {
        let truncated = |amount: i128| amount / 2;
        let report = DifferentialHarness::new().diff(token_amount(), truncated, reference_fee);

        let failure = report.failure().expect("half-up and truncated differ");
        assert_eq!(failure.divergence().delta(), Some(-1));
        assert_eq!(failure.divergence().kind(), &DivergenceKind::Return);

        // The payload has to reproduce on its own, since that is what it is for:
        // paste it into a regression test and it fails the same way. Only odd
        // amounts lose the rounding term, so a payload that is not odd proves
        // nothing about the bug. Shrinking stops at the smallest input it could
        // reach, which for a predicate as jagged as parity is not the global
        // minimum.
        let amount: i128 = failure.input().parse().expect("an i128 payload");
        assert_eq!(amount % 2, 1, "{amount} is not an odd amount");
        assert_eq!(truncated(amount) + 1, reference_fee(amount));
    }

    #[test]
    fn a_tolerance_of_one_forgives_the_rounding_term() {
        let truncated = DifferentialHarness::new().diff(
            token_amount(),
            |amount: i128| amount / 2,
            reference_fee,
        );
        let tolerated = DifferentialHarness::new().tolerance(1).diff(
            token_amount(),
            |amount: i128| amount / 2,
            reference_fee,
        );

        assert!(!truncated.is_match());
        assert!(tolerated.is_match(), "{:?}", tolerated.failure());
    }

    #[test]
    fn shrinking_finds_the_smallest_input_that_breaks_the_model() {
        // Everything from 43 up diverges, so the answer is 43 exactly. A harness
        // that reported the first failure instead would print some large amount
        // that says nothing about the bug.
        let report = DifferentialHarness::new().diff(
            any::<u32>(),
            |value| value,
            |value: u32| if value < 43 { value } else { value + 1 },
        );

        let failure = report.failure().expect("43 and above diverge");
        assert_eq!(failure.input(), "43");
        assert_eq!(failure.divergence().delta(), Some(-1));
    }

    #[test]
    fn a_seed_replays_the_same_generated_inputs() {
        // A shrunk payload already pins the failing case, so the seed only shows
        // itself in the inputs the run walked through on the way there. Record
        // them, because that is the only thing a seed is for.
        use std::cell::RefCell;

        let run = |seed| {
            let seen = RefCell::new(Vec::new());
            let report = DifferentialHarness::new().cases(16).seed(seed).diff(
                0i128..1_000_000i128,
                |value| {
                    seen.borrow_mut().push(value);
                    value
                },
                |value| value,
            );
            assert!(report.is_match());
            seen.into_inner()
        };

        let first = run(7);
        let second = run(7);
        assert_eq!(first.len(), 16, "every case must generate one input");
        assert_eq!(first, second, "the same seed must replay input for input");
        assert!(
            first.windows(2).any(|pair| pair[0] != pair[1]),
            "a fixed seed must not collapse the run onto one value"
        );
    }

    #[test]
    fn state_divergence_names_the_key_that_moved() {
        let env = Env::default();
        let id = env.register(Fees, ());
        let client = FeesClient::new(&env, &id);

        // Both sides call the same contract, so the return values cannot be what
        // fails: only the state the first side expects differs.
        let report = DifferentialHarness::new().cases(32).diff(
            token_amount(),
            |amount| {
                let fee = client.calculate_fee(&amount);
                ExecutionTrace::new(fee).with_state("fee", fee + 1)
            },
            |amount| {
                let fee = client.calculate_fee(&amount);
                ExecutionTrace::new(fee).with_state("fee", fee)
            },
        );

        let failure = report.failure().expect("the two states disagree");
        assert_eq!(
            failure.divergence().kind(),
            &DivergenceKind::State {
                key: "fee".to_string()
            }
        );
        assert_eq!(failure.divergence().delta(), Some(1));
        assert_eq!(
            failure.input(),
            "0",
            "every amount fails, so it shrinks to 0"
        );
        assert_eq!(failure.divergence().contract_value(), &DiffValue::Signed(1));
        assert_eq!(
            failure.divergence().reference_value(),
            &DiffValue::Signed(0)
        );
    }

    #[test]
    fn a_key_one_side_never_writes_is_a_divergence() {
        let report = DifferentialHarness::new().diff(
            any::<bool>(),
            |set: bool| {
                let mut trace = ExecutionTrace::new(0i32);
                if set {
                    trace = trace.with_state("written", 1i32);
                }
                trace
            },
            |_: bool| ExecutionTrace::new(0i32).with_state("written", 1i32),
        );

        let failure = report.failure().expect("one side skips the write");
        assert_eq!(failure.input(), "false");
        assert_eq!(failure.divergence().contract_value(), &DiffValue::Unset);
        assert_eq!(
            failure.divergence().reference_value(),
            &DiffValue::Signed(1)
        );
        assert_eq!(
            failure.divergence().delta(),
            None,
            "an absent key is not one unit away from a present one"
        );
    }

    #[test]
    fn a_tolerance_never_forgives_a_change_of_type() {
        let report = DifferentialHarness::new().tolerance(100).diff(
            any::<bool>(),
            DiffValue::Bool,
            |value: bool| DiffValue::Unsigned(u128::from(value)),
        );

        let failure = report.failure().expect("true is not 1 to this harness");
        assert_eq!(failure.divergence().delta(), None);
        assert_eq!(failure.input(), "false");
        assert!(
            failure.render(100).contains("contract false, model 0"),
            "{}",
            failure.render(100)
        );
    }

    #[test]
    fn an_amm_invariant_holds_against_its_reference_curve() {
        let env = Env::default();
        let id = env.register(Fees, ());
        let client = FeesClient::new(&env, &id);

        // Restated from the constant-product spec rather than copied from the
        // contract body, which is the whole point of the exercise.
        fn reference_amount_out(dx: i128, x: i128, y: i128) -> i128 {
            let dx_with_fee = dx * 997;
            (dx_with_fee * y) / (x * 1000 + dx_with_fee)
        }

        let (x, y) = (1_000_000i128, 2_000_000i128);
        let agreeing = DifferentialHarness::new().cases(48).diff(
            1i128..500_000i128,
            |dx| client.amount_out(&dx, &x, &y),
            |dx| reference_amount_out(dx, x, y),
        );
        assert!(agreeing.is_match(), "{:?}", agreeing.failure());

        // Forget that the fee comes out before the product is taken and every
        // output is one unit short. The harness must see it.
        let short = DifferentialHarness::new().cases(48).diff(
            1i128..500_000i128,
            |dx| client.amount_out(&dx, &x, &y),
            |dx| reference_amount_out(dx, x, y) + 1,
        );
        let failure = short.failure().expect("one unit of output is a divergence");
        assert_eq!(failure.divergence().delta(), Some(-1));
        assert_eq!(failure.input(), "1", "the cheapest swap that shows it");
    }

    #[test]
    fn compare_panics_naming_the_payload_and_both_sides() {
        let report =
            DifferentialHarness::new().diff(any::<u32>(), |value| value, |value| value / 2);
        let failure = report
            .failure()
            .expect("a third of a number is not the number");

        let text = failure.render(report.tolerance());
        assert!(text.contains("smallest failing input: 1"), "{text}");
        assert!(text.contains("return value: contract 1, model 0"), "{text}");
        assert!(text.contains("tolerance: 0"), "{text}");

        let captured = std::panic::catch_unwind(|| report.assert_matches());
        assert!(captured.is_err(), "a divergence must stop the test");
    }

    #[test]
    fn every_primitive_a_contract_returns_reduces_to_a_comparable_value() {
        assert_eq!(DiffValue::from(7i128), DiffValue::Signed(7));
        assert_eq!(DiffValue::from(7u32), DiffValue::Unsigned(7));
        assert_eq!(
            DiffValue::from(true),
            DiffValue::Bool(true),
            "a boolean is not the number 1"
        );
        assert_eq!(DiffValue::from(vec![1u8, 2]), DiffValue::Bytes(vec![1, 2]));
        assert_eq!(DiffValue::from("hi"), DiffValue::Text("hi".to_string()));
        assert_eq!(DiffValue::from(Some(1i64)), DiffValue::Signed(1));
        assert_eq!(DiffValue::from(None::<i64>), DiffValue::None);
        assert_eq!(DiffValue::from(()), DiffValue::None);
        assert_eq!(
            DiffValue::list([DiffValue::from(1i8), DiffValue::from(2i8)]),
            DiffValue::List(vec![DiffValue::Signed(1), DiffValue::Signed(2)])
        );

        assert_eq!(7i128.into_trace().result(), &DiffValue::Signed(7));
        assert!(
            ExecutionTrace::new(0i8).state().is_empty(),
            "a bare result carries no state"
        );
    }

    #[test]
    fn a_u128_beyond_the_signed_range_has_no_delta() {
        // `i128::MAX + 1` has no signed form, so the harness reports the two sides
        // as different without inventing a difference between them.
        let beyond = DiffValue::Unsigned(i128::MAX as u128 + 1);
        assert_eq!(beyond.as_i128(), None);
        assert_eq!(
            DiffValue::Unsigned(i128::MAX as u128).as_i128(),
            Some(i128::MAX)
        );
    }
}
