//! Event payload fuzzing harness.
//!
//! Indexers and downstream contracts deserialize contract events, so event
//! parsers must reject malformed payloads without panicking. This module
//! produces valid `ContractEvent` values and mutates them using a set of
//! operators that emulate realistic corruption (missing fields, wrong types,
//! truncated bytes and flipped signs).
//!
//! ```
//! use soroban_testkit_generators::event_fuzz::{valid_transfer_event, EventFuzzHarness};
//!
//! let harness = EventFuzzHarness::new(valid_transfer_event());
//! let event = harness.mutate(soroban_testkit_generators::event_fuzz::MutationOperator::DropField, 7);
//! assert_ne!(&event, harness.base_event());
//! ```

use soroban_sdk::xdr::{
    BytesM, ContractEvent, ContractEventBody, ContractEventType, ContractEventV0, ContractId,
    ExtensionPoint, Hash, Int128Parts, ScBytes, ScString, ScSymbol, ScVal, ScVec, StringM, VecM,
};

/// A deterministic mutation that can be applied to a valid event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationOperator {
    /// Remove a topic, or blank the data when no topics remain.
    DropField,
    /// Replace a value with a different `ScVal` variant.
    TypeShift,
    /// Halve the length of a byte-bearing value.
    ByteTruncate,
    /// Flip the sign of a numeric value, or toggle a boolean.
    SignFlip,
}

impl MutationOperator {
    /// Every operator supported by the harness.
    pub const ALL: [MutationOperator; 4] = [
        MutationOperator::DropField,
        MutationOperator::TypeShift,
        MutationOperator::ByteTruncate,
        MutationOperator::SignFlip,
    ];

    /// Human-readable operator name.
    pub fn name(self) -> &'static str {
        match self {
            MutationOperator::DropField => "drop-field",
            MutationOperator::TypeShift => "type-shift",
            MutationOperator::ByteTruncate => "byte-truncate",
            MutationOperator::SignFlip => "sign-flip",
        }
    }
}

/// Builds valid events and applies deterministic mutations to them.
#[derive(Debug, Clone)]
pub struct EventFuzzHarness {
    base: ContractEvent,
}

impl EventFuzzHarness {
    /// Create a harness around a known-valid event.
    pub fn new(base: ContractEvent) -> Self {
        Self { base }
    }

    /// The unmodified event used as the mutation seed.
    pub fn base_event(&self) -> &ContractEvent {
        &self.base
    }

    /// Apply a single mutation operator with a deterministic seed.
    pub fn mutate(&self, operator: MutationOperator, seed: u64) -> ContractEvent {
        let mut event = self.base.clone();
        apply_operator(&mut event, operator, seed);
        event
    }

    /// Apply every operator in sequence, returning one event per operator.
    pub fn mutate_all(&self, seed: u64) -> Vec<ContractEvent> {
        MutationOperator::ALL
            .iter()
            .map(|operator| self.mutate(*operator, seed))
            .collect()
    }

    /// Deterministically reduce an event while preserving its structure.
    ///
    /// This mirrors the shrinking phase of a property test: topics are dropped
    /// first, then byte-bearing values are halved and numeric values are
    /// zeroed. The result is always at most as large as the input.
    pub fn shrink(&self, event: &ContractEvent) -> ContractEvent {
        let mut current = event.clone();
        for _ in 0..32 {
            let next = shrink_once(&current);
            if next == current {
                break;
            }
            current = next;
        }
        current
    }

    /// All intermediate events produced while shrinking `event`.
    pub fn shrink_candidates(&self, event: &ContractEvent) -> Vec<ContractEvent> {
        let mut candidates = Vec::new();
        let mut current = event.clone();
        for _ in 0..32 {
            let next = shrink_once(&current);
            if next == current {
                break;
            }
            candidates.push(next.clone());
            current = next;
        }
        candidates
    }

    /// A `proptest` strategy that yields mutated events.
    #[cfg(feature = "proptest")]
    pub fn strategy(&self) -> impl proptest::strategy::Strategy<Value = ContractEvent> {
        fuzzed_event(self.base.clone())
    }
}

/// Create a well-formed transfer event suitable for mutation.
pub fn valid_transfer_event() -> ContractEvent {
    let topics = VecM::try_from(vec![symbol("transfer")]).expect("one topic fits");
    let data = ScVal::I128(Int128Parts {
        hi: 0,
        lo: 1_000_000,
    });

    ContractEvent {
        ext: ExtensionPoint::V0,
        contract_id: Some(ContractId(Hash([7u8; 32]))),
        type_: ContractEventType::Contract,
        body: ContractEventBody::V0(ContractEventV0 { topics, data }),
    }
}

/// Free-function strategy matching the ergonomic API from the issue.
#[cfg(feature = "proptest")]
pub fn fuzzed_event(
    base: ContractEvent,
) -> impl proptest::strategy::Strategy<Value = ContractEvent> {
    use proptest::prelude::*;

    (0u8..4u8, any::<u64>()).prop_map(move |(index, seed)| {
        let operator = MutationOperator::ALL[index as usize];
        EventFuzzHarness::new(base.clone()).mutate(operator, seed)
    })
}

fn symbol(value: &str) -> ScVal {
    ScVal::Symbol(ScSymbol(StringM::try_from(value).expect("valid symbol")))
}

fn apply_operator(event: &mut ContractEvent, operator: MutationOperator, seed: u64) {
    let ContractEventBody::V0(body) = &mut event.body;
    match operator {
        MutationOperator::DropField => drop_field(body),
        MutationOperator::TypeShift => {
            mutate_first(body, seed, shift_value);
        }
        MutationOperator::ByteTruncate => {
            mutate_first(body, seed, truncate_value);
        }
        MutationOperator::SignFlip => {
            mutate_first(body, seed, flip_sign);
        }
    }
}

fn drop_field(body: &mut ContractEventV0) {
    let mut topics = body.topics.to_vec();
    if topics.pop().is_none() {
        body.data = ScVal::Void;
        return;
    }
    body.topics = VecM::try_from(topics).expect("topics fit");
}

fn mutate_first(
    body: &mut ContractEventV0,
    seed: u64,
    transform: fn(&ScVal, u64) -> ScVal,
) -> bool {
    let mut topics = body.topics.to_vec();
    if !topics.is_empty() {
        for offset in 0..topics.len() {
            let index = (seed as usize + offset) % topics.len();
            let transformed = transform(&topics[index], seed);
            if transformed != topics[index] {
                topics[index] = transformed;
                body.topics = VecM::try_from(topics).expect("topics fit");
                return true;
            }
        }
    }
    let transformed = transform(&body.data, seed);
    if transformed != body.data {
        body.data = transformed;
    }
    false
}

fn shift_value(value: &ScVal, _seed: u64) -> ScVal {
    match value {
        ScVal::U32(v) => ScVal::I32(*v as i32),
        ScVal::I32(v) => ScVal::U32(v.unsigned_abs()),
        ScVal::U64(v) => ScVal::I64(*v as i64),
        ScVal::I64(v) => ScVal::U64(v.unsigned_abs()),
        ScVal::U128(_) => ScVal::I128(Int128Parts { hi: -1, lo: 1 }),
        ScVal::I128(_) => ScVal::U128(soroban_sdk::xdr::UInt128Parts { hi: 0, lo: 1 }),
        ScVal::Symbol(_) => ScVal::String(ScString(
            StringM::try_from("mutated").expect("valid string"),
        )),
        ScVal::String(_) => symbol("mutated"),
        ScVal::Bytes(_) => ScVal::U32(0),
        ScVal::Bool(v) => ScVal::U32(u32::from(*v)),
        _ => ScVal::Void,
    }
}

fn truncate_value(value: &ScVal, _seed: u64) -> ScVal {
    match value {
        ScVal::Bytes(bytes) => {
            let mut data = bytes.0.as_vec().clone();
            data.truncate(data.len() / 2);
            ScVal::Bytes(ScBytes(
                BytesM::try_from(data).expect("truncated bytes fit"),
            ))
        }
        ScVal::String(text) => {
            truncated_string(&text.0).map_or(ScVal::Void, |s| ScVal::String(ScString(s)))
        }
        ScVal::Symbol(text) => {
            truncated_string(&text.0).map_or(ScVal::Void, |s| ScVal::Symbol(ScSymbol(s)))
        }
        ScVal::Vec(Some(items)) => {
            let mut values = items.0.to_vec();
            values.truncate(values.len() / 2);
            ScVal::Vec(Some(ScVec(
                VecM::try_from(values).expect("truncated vec fits"),
            )))
        }
        other => other.clone(),
    }
}

fn truncated_string<const MAX: u32>(text: &StringM<MAX>) -> Option<StringM<MAX>> {
    let inner: &Vec<u8> = text.as_ref();
    let mut bytes = inner.clone();
    if bytes.is_empty() {
        return None;
    }
    bytes.truncate(bytes.len() / 2);
    StringM::try_from(bytes).ok()
}

fn flip_sign(value: &ScVal, _seed: u64) -> ScVal {
    match value {
        ScVal::I32(v) => ScVal::I32(v.saturating_neg()),
        ScVal::I64(v) => ScVal::I64(v.saturating_neg()),
        ScVal::I128(parts) => {
            let value = ((parts.hi as i128) << 64) | parts.lo as i128;
            let flipped = value.saturating_neg();
            ScVal::I128(Int128Parts {
                hi: (flipped >> 64) as i64,
                lo: flipped as u64,
            })
        }
        ScVal::Bool(v) => ScVal::Bool(!v),
        other => other.clone(),
    }
}

fn shrink_once(event: &ContractEvent) -> ContractEvent {
    let mut next = event.clone();
    let ContractEventBody::V0(body) = &mut next.body;

    let mut topics = body.topics.to_vec();
    if topics.len() > 1 {
        topics.pop();
        body.topics = VecM::try_from(topics).expect("topics fit");
        return next;
    }

    if let Some(topic) = topics.first_mut() {
        let reduced = minimize_value(topic);
        if reduced != *topic {
            *topic = reduced;
            body.topics = VecM::try_from(topics).expect("topics fit");
            return next;
        }
    }

    let reduced = minimize_value(&body.data);
    if reduced != body.data {
        body.data = reduced;
        return next;
    }

    if topics.len() == 1 {
        topics.clear();
        body.topics = VecM::try_from(topics).expect("topics fit");
    }

    next
}

fn minimize_value(value: &ScVal) -> ScVal {
    match value {
        ScVal::Bytes(bytes) if !bytes.0.is_empty() => truncate_value(value, 0),
        ScVal::String(_) | ScVal::Symbol(_) => truncate_value(value, 0),
        ScVal::Vec(Some(items)) if !items.0.is_empty() => {
            let mut values = items.0.to_vec();
            values.truncate(values.len() / 2);
            ScVal::Vec(Some(ScVec(VecM::try_from(values).expect("vec fits"))))
        }
        ScVal::U32(v) if *v != 0 => ScVal::U32(0),
        ScVal::I32(v) if *v != 0 => ScVal::I32(0),
        ScVal::U64(v) if *v != 0 => ScVal::U64(0),
        ScVal::I64(v) if *v != 0 => ScVal::I64(0),
        ScVal::I128(_) => ScVal::I128(Int128Parts { hi: 0, lo: 0 }),
        ScVal::U128(_) => ScVal::U128(soroban_sdk::xdr::UInt128Parts { hi: 0, lo: 0 }),
        ScVal::Bool(true) => ScVal::Bool(false),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_transfer(event: &ContractEvent) -> Result<u64, &'static str> {
        let ContractEventBody::V0(body) = &event.body;
        let topic = body.topics.first().ok_or("missing topic")?;
        match topic {
            ScVal::Symbol(name) if name.0.to_string() == "transfer" => {}
            ScVal::Symbol(_) => return Err("unexpected topic"),
            _ => return Err("topic is not a symbol"),
        }
        match &body.data {
            ScVal::I128(parts) => Ok(parts.lo),
            _ => Err("data is not i128"),
        }
    }

    #[test]
    fn valid_event_parses() {
        assert_eq!(parse_transfer(&valid_transfer_event()), Ok(1_000_000));
    }

    #[test]
    fn every_operator_produces_a_distinct_event() {
        let harness = EventFuzzHarness::new(valid_transfer_event());
        for operator in MutationOperator::ALL {
            let mutated = harness.mutate(operator, 3);
            assert_ne!(mutated, *harness.base_event(), "{operator:?}");
        }
    }

    #[test]
    fn drop_field_is_rejected_by_parser() {
        let harness = EventFuzzHarness::new(valid_transfer_event());
        let event = harness.mutate(MutationOperator::DropField, 0);
        assert!(parse_transfer(&event).is_err());
    }

    #[test]
    fn type_shift_is_rejected_by_parser() {
        let harness = EventFuzzHarness::new(valid_transfer_event());
        let event = harness.mutate(MutationOperator::TypeShift, 0);
        assert!(parse_transfer(&event).is_err());
    }

    #[test]
    fn shrinking_reduces_payload_size() {
        let harness = EventFuzzHarness::new(valid_transfer_event());
        let event = harness.mutate(MutationOperator::ByteTruncate, 1);
        let shrunk = harness.shrink(&event);
        assert!(!harness.shrink_candidates(&event).is_empty());
        assert_eq!(harness.shrink(&shrunk), shrunk);
    }

    #[test]
    fn all_mutations_are_deterministic() {
        let harness = EventFuzzHarness::new(valid_transfer_event());
        for operator in MutationOperator::ALL {
            assert_eq!(harness.mutate(operator, 42), harness.mutate(operator, 42));
        }
    }

    #[cfg(feature = "proptest")]
    proptest::proptest! {
        #[test]
        fn fuzzed_event_parser_never_panics(
            event in fuzzed_event(valid_transfer_event()),
        ) {
            let _ = parse_transfer(&event);
        }
    }
}
