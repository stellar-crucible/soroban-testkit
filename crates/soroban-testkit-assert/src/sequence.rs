//! Event sequence assertions for verifying exact and interleaved event flows.
//!
//! While [`crate::events::EventLog`] collects events across multiple contract invocations,
//! verifying that events occurred in an exact sequence—or that a specific sequence occurred
//! with allowed interleaving—is streamlined by [`assert_event_sequence!`].

use crate::events::{emitting_contract, event_data, has_topic, EventData, EventLog};
use soroban_sdk::xdr::ContractEvent;
use soroban_sdk::{Address, Env};

/// The sequence matching strategy for [`assert_event_sequence!`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SequenceMode {
    /// Strict sequential matching: the observed events must match the expected sequence
    /// exactly in length and order without any unlisted or interleaved intermediate events.
    Strict,
    /// Relaxed sequential matching: each expected event must occur after the previous one,
    /// allowing unrelated intermediate events between them (ordered subsequence).
    Relaxed,
}

pub use SequenceMode::{Relaxed, Strict};

/// Alias for `SequenceMode::Strict`.
pub const EXACT_SEQUENCE: SequenceMode = SequenceMode::Strict;
/// Alias for `SequenceMode::Relaxed`.
pub const ORDERED_SUBSEQUENCE: SequenceMode = SequenceMode::Relaxed;

/// Type alias for event data predicate closures.
pub type EventDataPredicate = Box<dyn Fn(&EventData<'_>) -> bool>;

/// An expectation against a single emitted event in a sequence.
pub struct ExpectedEvent {
    /// Expected topic symbol.
    pub topic: Option<String>,
    /// Expected emitting contract address.
    pub contract: Option<Address>,
    /// Optional closure verifying event payload data.
    pub data_predicate: Option<EventDataPredicate>,
    /// Optional descriptive label for failure diagnostics.
    pub label: Option<String>,
}

impl ExpectedEvent {
    /// Creates a blank expectation matching any event.
    pub fn new() -> Self {
        Self {
            topic: None,
            contract: None,
            data_predicate: None,
            label: None,
        }
    }

    /// Restricts match to events carrying `topic` as a topic symbol.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Restricts match to events emitted by `contract`.
    pub fn with_contract(mut self, contract: &Address) -> Self {
        self.contract = Some(contract.clone());
        self
    }

    /// Restricts match to events whose payload satisfies `predicate`.
    pub fn with_data(mut self, predicate: impl Fn(&EventData<'_>) -> bool + 'static) -> Self {
        self.data_predicate = Some(Box::new(predicate));
        self
    }

    /// Assigns a human-readable label to this expectation.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Checks whether `event` satisfies all configured filters on this expectation.
    pub fn matches(&self, env: &Env, event: &ContractEvent) -> bool {
        if let Some(topic) = &self.topic {
            if !has_topic(event, topic) {
                return false;
            }
        }

        if let Some(contract) = &self.contract {
            let wanted = emitting_contract(contract);
            match (wanted.as_ref(), event.contract_id.as_ref()) {
                (Some(w), Some(c)) if w == c => {}
                _ => return false,
            }
        }

        if let Some(pred) = &self.data_predicate {
            let data = EventData::new(env, event_data(event));
            if !pred(&data) {
                return false;
            }
        }

        true
    }

    /// Formats this expectation for error diagnostics.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(lbl) = &self.label {
            parts.push(format!("label: \"{lbl}\""));
        }
        if let Some(topic) = &self.topic {
            parts.push(format!("topic: \"{topic}\""));
        }
        if let Some(contract) = &self.contract {
            parts.push(format!("contract: {contract:?}"));
        }
        if self.data_predicate.is_some() {
            parts.push("data: <predicate>".to_string());
        }
        if parts.is_empty() {
            "{ any event }".to_string()
        } else {
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

impl Default for ExpectedEvent {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper trait allowing [`assert_event_sequence!`] to accept [`EventLog`], `&EventLog`, or `Env`.
pub trait AsEventSlice {
    fn env(&self) -> &Env;
    fn events(&self) -> &[ContractEvent];
}

impl<'a> AsEventSlice for EventLog<'a> {
    fn env(&self) -> &Env {
        EventLog::env(self)
    }
    fn events(&self) -> &[ContractEvent] {
        EventLog::events(self)
    }
}

impl<T: AsEventSlice + ?Sized> AsEventSlice for &T {
    fn env(&self) -> &Env {
        (**self).env()
    }
    fn events(&self) -> &[ContractEvent] {
        (**self).events()
    }
}

/// Core sequence assertion implementation.
pub fn assert_event_sequence_impl(
    source: &impl AsEventSlice,
    expected: &[ExpectedEvent],
    mode: SequenceMode,
) {
    let env = source.env();
    let events = source.events();

    match mode {
        SequenceMode::Strict => assert_strict(env, events, expected),
        SequenceMode::Relaxed => assert_relaxed(env, events, expected),
    }
}

fn assert_strict(env: &Env, events: &[ContractEvent], expected: &[ExpectedEvent]) {
    if events.len() != expected.len() {
        let mut report = format!(
            "Event sequence length mismatch (mode = Strict / exact_sequence):\n\
             Expected {} events, but observed {} events.\n\n\
             Sequence breakdown:\n",
            expected.len(),
            events.len()
        );

        let max_len = events.len().max(expected.len());
        for i in 0..max_len {
            let exp_desc = expected
                .get(i)
                .map(|e| e.describe())
                .unwrap_or_else(|| "<none (unexpected)>".to_string());
            let obs_desc = events
                .get(i)
                .map(|e| describe_event(env, e))
                .unwrap_or_else(|| "<none (missing)>".to_string());

            let status = match (expected.get(i), events.get(i)) {
                (Some(exp), Some(obs)) => {
                    if exp.matches(env, obs) {
                        "✓ MATCHED"
                    } else {
                        "✗ DIVERGED"
                    }
                }
                (Some(_), None) => "✗ MISSING",
                (None, Some(_)) => "✗ UNEXPECTED",
                (None, None) => " ",
            };

            report.push_str(&format!(
                "  [{i:>2}] {status:<12} | Expected: {exp_desc}\n\
                               | Observed: {obs_desc}\n"
            ));
        }

        panic!("{report}");
    }

    for (i, (exp, obs)) in expected.iter().zip(events.iter()).enumerate() {
        if !exp.matches(env, obs) {
            let mut report = format!(
                "Event sequence diverged at index {i} (mode = Strict / exact_sequence):\n\
                 Expected event: {}\n\
                 Observed event: {}\n\n\
                 Full sequence breakdown:\n",
                exp.describe(),
                describe_event(env, obs)
            );

            for (j, (e, o)) in expected.iter().zip(events.iter()).enumerate() {
                let status = if j < i {
                    "✓ MATCHED"
                } else if j == i {
                    "✗ DIVERGED"
                } else {
                    "  PENDING"
                };
                report.push_str(&format!(
                    "  [{j:>2}] {status:<10} | Expected: {}\n\
                               | Observed: {}\n",
                    e.describe(),
                    describe_event(env, o)
                ));
            }

            panic!("{report}");
        }
    }
}

fn assert_relaxed(env: &Env, events: &[ContractEvent], expected: &[ExpectedEvent]) {
    if expected.is_empty() {
        return;
    }

    let mut exp_idx = 0;
    let mut matched_pairs: Vec<(usize, usize)> = Vec::new(); // (expected_idx, event_idx)

    for (ev_idx, event) in events.iter().enumerate() {
        if exp_idx < expected.len() && expected[exp_idx].matches(env, event) {
            matched_pairs.push((exp_idx, ev_idx));
            exp_idx += 1;
            if exp_idx == expected.len() {
                break;
            }
        }
    }

    if exp_idx < expected.len() {
        let first_missing = &expected[exp_idx];
        let last_matched_ev = matched_pairs.last().map(|&(_, ev_idx)| ev_idx);

        let mut report = format!(
            "Event sequence relaxed match failed (mode = Relaxed / ordered_subsequence):\n\
             Matched {} of {} expected events before divergence.\n\
             First missing expected event [{}]: {}\n\n\
             Progress:\n",
            exp_idx,
            expected.len(),
            exp_idx,
            first_missing.describe()
        );

        for (i, exp) in expected.iter().enumerate() {
            if let Some(&(_, ev_idx)) = matched_pairs.iter().find(|(e, _)| *e == i) {
                report.push_str(&format!(
                    "  [{i:>2}] ✓ MATCHED at log index {ev_idx}: {}\n",
                    exp.describe()
                ));
            } else if i == exp_idx {
                report.push_str(&format!(
                    "  [{i:>2}] ✗ MISSING (not found after log index {:?}): {}\n",
                    last_matched_ev,
                    exp.describe()
                ));
            } else {
                report.push_str(&format!("  [{i:>2}]   PENDING: {}\n", exp.describe()));
            }
        }

        report.push_str("\nObserved event log:\n");
        if events.is_empty() {
            report.push_str("  (event log is empty)\n");
        } else {
            for (idx, ev) in events.iter().enumerate() {
                let mark = if matched_pairs.iter().any(|(_, ev_idx)| *ev_idx == idx) {
                    "✓ [matched]"
                } else {
                    "  [skipped]"
                };
                report.push_str(&format!(
                    "  [{idx:>2}] {mark:<11} {}\n",
                    describe_event(env, ev)
                ));
            }
        }

        panic!("{report}");
    }
}

fn describe_event(_env: &Env, event: &ContractEvent) -> String {
    let topics = crate::events::event_topics(event);
    let contract_str = event
        .contract_id
        .as_ref()
        .map(|c| {
            let mut s = String::new();
            for b in c.0.as_slice() {
                use std::fmt::Write as _;
                let _ = write!(&mut s, "{:02x}", b);
            }
            s
        })
        .unwrap_or_else(|| "none".to_string());

    format!(
        "topics: [{topics}], contract: {contract_str}",
        topics = topics.join(", ")
    )
}

#[doc(hidden)]
#[macro_export]
macro_rules! __event_seq_build_field {
    ($item:expr, topic, $val:expr) => {
        $item.with_topic($val)
    };
    ($item:expr, contract, $val:expr) => {
        $item.with_contract(&$val)
    };
    ($item:expr, data, $val:expr) => {
        $item.with_data($val)
    };
    ($item:expr, label, $val:expr) => {
        $item.with_label($val)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __event_seq_item {
    ({ $($field:ident : $val:expr),* $(,)? }) => {{
        let mut __item = $crate::sequence::ExpectedEvent::new();
        $(
            __item = $crate::__event_seq_build_field!(__item, $field, $val);
        )*
        __item
    }};
    ($expr:expr) => {
        $expr
    };
}

/// Asserts that a sequence of events occurred matching expectations.
///
/// Supports both **Strict** (exact sequence: length and elements must match 1:1)
/// and **Relaxed** (ordered subsequence: expected events occur in order with intermediate
/// events permitted).
///
/// # Example
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_event_sequence;
/// use soroban_testkit_assert::sequence::Relaxed;
///
/// assert_event_sequence!(event_log, [
///     { topic: "Deposit", contract: vault_id },
///     { topic: "Mint", contract: token_id },
///     { topic: "Transfer", contract: token_id },
/// ], mode = Relaxed);
/// ```
#[macro_export]
macro_rules! assert_event_sequence {
    ($source:expr, [ $( $item:tt ),* $(,)? ], mode = $mode:ident $(,)?) => {{
        let __expected = vec![ $( $crate::__event_seq_item!($item) ),* ];
        let __mode = $crate::sequence::SequenceMode::$mode;
        $crate::sequence::assert_event_sequence_impl(&$source, &__expected, __mode)
    }};
    ($source:expr, [ $( $item:tt ),* $(,)? ], mode = $mode:expr $(,)?) => {{
        let __expected = vec![ $( $crate::__event_seq_item!($item) ),* ];
        $crate::sequence::assert_event_sequence_impl(&$source, &__expected, $mode)
    }};
    ($source:expr, [ $( $item:tt ),* $(,)? ] $(,)?) => {{
        let __expected = vec![ $( $crate::__event_seq_item!($item) ),* ];
        $crate::sequence::assert_event_sequence_impl(&$source, &__expected, $crate::sequence::SequenceMode::Strict)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, symbol_short, Env};

    #[contract]
    pub struct Emitter;

    #[contractimpl]
    #[allow(deprecated)]
    impl Emitter {
        pub fn emit_single(env: Env, topic: soroban_sdk::Symbol) {
            env.events().publish((topic,), 1u32);
        }

        pub fn emit_three(env: Env) {
            env.events().publish((symbol_short!("step1"),), 100u32);
            env.events().publish((symbol_short!("step2"),), 200u32);
            env.events().publish((symbol_short!("step3"),), 300u32);
        }

        pub fn emit_interleaved(env: Env) {
            env.events().publish((symbol_short!("step1"),), 100u32);
            env.events().publish((symbol_short!("noise"),), 999u32);
            env.events().publish((symbol_short!("step2"),), 200u32);
            env.events().publish((symbol_short!("noise"),), 999u32);
            env.events().publish((symbol_short!("step3"),), 300u32);
        }
    }

    #[test]
    fn test_strict_exact_sequence_success() {
        let env = Env::default();
        let id = env.register(Emitter, ());
        let client = EmitterClient::new(&env, &id);

        let mut log = EventLog::new(&env);
        client.emit_three();
        log.collect();

        assert_event_sequence!(
            log,
            [
                { topic: "step1", contract: id },
                { topic: "step2", contract: id },
                { topic: "step3", contract: id },
            ],
            mode = Strict
        );
    }

    #[test]
    fn test_relaxed_subsequence_with_interleaving() {
        let env = Env::default();
        let id = env.register(Emitter, ());
        let client = EmitterClient::new(&env, &id);

        let mut log = EventLog::new(&env);
        client.emit_interleaved();
        log.collect();

        assert_event_sequence!(
            log,
            [
                { topic: "step1", contract: id },
                { topic: "step2", contract: id },
                { topic: "step3", contract: id },
            ],
            mode = Relaxed
        );
    }

    #[test]
    #[should_panic(expected = "Event sequence length mismatch")]
    fn test_strict_fails_on_interleaving() {
        let env = Env::default();
        let id = env.register(Emitter, ());
        let client = EmitterClient::new(&env, &id);

        let mut log = EventLog::new(&env);
        client.emit_interleaved();
        log.collect();

        assert_event_sequence!(
            log,
            [
                { topic: "step1", contract: id },
                { topic: "step2", contract: id },
                { topic: "step3", contract: id },
            ],
            mode = Strict
        );
    }

    #[test]
    #[should_panic(expected = "Event sequence diverged at index")]
    fn test_strict_out_of_order_fails() {
        let env = Env::default();
        let id = env.register(Emitter, ());
        let client = EmitterClient::new(&env, &id);

        let mut log = EventLog::new(&env);
        client.emit_three();
        log.collect();

        assert_event_sequence!(
            log,
            [
                { topic: "step2", contract: id },
                { topic: "step1", contract: id },
                { topic: "step3", contract: id },
            ],
            mode = Strict
        );
    }

    #[test]
    #[should_panic(expected = "Event sequence relaxed match failed")]
    fn test_relaxed_out_of_order_fails() {
        let env = Env::default();
        let id = env.register(Emitter, ());
        let client = EmitterClient::new(&env, &id);

        let mut log = EventLog::new(&env);
        client.emit_three();
        log.collect();

        assert_event_sequence!(
            log,
            [
                { topic: "step3", contract: id },
                { topic: "step1", contract: id },
            ],
            mode = Relaxed
        );
    }

    #[test]
    fn test_empty_log_and_empty_expectation() {
        let env = Env::default();
        let log = EventLog::new(&env);

        assert_event_sequence!(log, [], mode = Strict);
        assert_event_sequence!(log, [], mode = Relaxed);
    }

    #[test]
    #[should_panic(expected = "Event sequence length mismatch")]
    fn test_empty_log_with_non_empty_expectation_strict() {
        let env = Env::default();
        let log = EventLog::new(&env);

        assert_event_sequence!(log, [{ topic: "missing" }], mode = Strict);
    }

    #[test]
    #[should_panic(expected = "Event sequence relaxed match failed")]
    fn test_empty_log_with_non_empty_expectation_relaxed() {
        let env = Env::default();
        let log = EventLog::new(&env);

        assert_event_sequence!(log, [{ topic: "missing" }], mode = Relaxed);
    }

    #[test]
    fn test_matching_with_data_predicate() {
        let env = Env::default();
        let id = env.register(Emitter, ());
        let client = EmitterClient::new(&env, &id);

        let mut log = EventLog::new(&env);
        client.emit_three();
        log.collect();

        assert_event_sequence!(
            log,
            [
                { topic: "step1", data: |d| d.deserialize::<u32>() == Some(100) },
                { topic: "step2", data: |d| d.deserialize::<u32>() == Some(200) },
                { topic: "step3", data: |d| d.deserialize::<u32>() == Some(300) },
            ],
            mode = Strict
        );
    }
}
