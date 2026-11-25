//! Dot-notation navigation into the payload of an emitted event.
//!
//! A contract that publishes a nested struct turns it into nested maps on the
//! ledger, so asserting on one leaf meant destructuring the payload by hand:
//!
//! ```text
//! EventMatcher::new(&env)
//!     .with_topic("order")
//!     .assert_data_matches(|data| {
//!         data.field("details")
//!             .and_then(|details| details.field("price"))
//!             .and_then(|price| price.deserialize::<i128>())
//!             == Some(500)
//!     });
//! ```
//!
//! A path says the same thing in one line, and when it does not resolve the
//! failure prints the whole payload tree and the segment that stopped it:
//!
//! ```text
//! assert_event_payload_field!(&env, {
//!     topic: "order",
//!     path: "details.price",
//!     equals: 500i128,
//! });
//!
//! assert_event_payload_field!(&env, {
//!     topic: "order",
//!     path: "details.royalty_bps",
//!     matches: |bps: u32| bps < 1_000,
//! });
//! ```
//!
//! Paths navigate maps (which is what a `contracttype` struct becomes, keyed by
//! field name) and vectors, to any depth. A numeric segment indexes a vector, so
//! `lots.1` and `lots[1]` are the same step. The leaf is converted to the type
//! the expectation names, which is what keeps `equals: 500u32` from silently
//! comparing against an `i128` on the other side of a `TryFromVal`.

use std::fmt;

use soroban_sdk::xdr::ScVal;
use soroban_sdk::{Address, Env, TryFromVal, Val};

use crate::events::{data_label, event_data, event_topics, EventMatcher};

/// Asserts one navigated field of an event payload.
///
/// Build it in steps when the filters are decided separately, or use
/// [`assert_event_payload_field!`](crate::assert_event_payload_field) to write
/// the topic, path and expectation in one block.
///
/// ```text
/// use soroban_testkit_assert::payload::PayloadAssertion;
///
/// PayloadAssertion::new(&env)
///     .with_topic("order")
///     .path("details.price")
///     .assert_equals(500i128);
/// ```
///
/// The scope is the same as [`EventMatcher`](crate::events::EventMatcher): the
/// invocation that has just run. The assertion passes when **any** event in
/// scope carries a payload whose path resolves and satisfies the expectation,
/// which is what makes one matcher usable over a topic that fires more than
/// once. When none does, every event in scope is reported.
pub struct PayloadAssertion<'a> {
    env: &'a Env,
    contract: Option<Address>,
    topic: Option<String>,
    path: Option<String>,
}

impl<'a> PayloadAssertion<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            contract: None,
            topic: None,
            path: None,
        }
    }

    /// Restricts the search to the events one contract published.
    pub fn from_contract(mut self, contract: &Address) -> Self {
        self.contract = Some(contract.clone());
        self
    }

    /// Restricts the search to events carrying this topic symbol.
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// The dot-separated path to the leaf field, e.g. `"order.details.price"`.
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The environment this assertion reads events from.
    pub fn env(&self) -> &'a Env {
        self.env
    }

    /// Asserts that the field at the configured path equals `expected`.
    ///
    /// The leaf is converted to `T` before the comparison, so a path that lands
    /// on a different type fails with the value it found and the type it could
    /// not become rather than with a confusing inequality.
    pub fn assert_equals<T>(&self, expected: T)
    where
        T: TryFromVal<Env, Val> + PartialEq + fmt::Debug,
    {
        let path = self.configured_path();
        let type_name = std::any::type_name::<T>();
        self.assert_leaf(
            &format!("{path:?} == {expected:?}"),
            |env, leaf| match read_as::<T>(env, leaf) {
                Some(actual) if actual == expected => Ok(()),
                Some(_) => Err(format!(
                    "resolved to {}, expected {expected:?}",
                    data_label(env, leaf)
                )),
                None => Err(format!(
                    "holds {}, which does not convert to {type_name}",
                    data_label(env, leaf)
                )),
            },
        );
    }

    /// Asserts that the field at the configured path satisfies `predicate`.
    ///
    /// The predicate receives the leaf already converted to the type its
    /// parameter names, so `|price: i128| price > 0` reads a price and nothing
    /// else; a leaf of another type fails as a conversion mismatch.
    pub fn assert_matches<T, F>(&self, predicate: F)
    where
        T: TryFromVal<Env, Val>,
        F: Fn(T) -> bool,
    {
        let path = self.configured_path();
        let type_name = std::any::type_name::<T>();
        self.assert_leaf(
            &format!("{path:?} matches the predicate"),
            |env, leaf| match read_as::<T>(env, leaf) {
                Some(actual) => {
                    if predicate(actual) {
                        Ok(())
                    } else {
                        Err(format!(
                            "resolved to {}, which the predicate rejected",
                            data_label(env, leaf)
                        ))
                    }
                }
                None => Err(format!(
                    "holds {}, which does not convert to {type_name}",
                    data_label(env, leaf)
                )),
            },
        );
    }

    /// Runs one expectation against every event in scope, reporting all of them
    /// when none satisfies it.
    fn assert_leaf<F>(&self, expectation: &str, check: F)
    where
        F: Fn(&Env, &ScVal) -> Result<(), String>,
    {
        let path = self.configured_path();
        let events = self.matcher().matched();

        if events.is_empty() {
            panic!(
                "assert_event_payload_field! failed: no event in scope, so {expectation} could not be checked\n\
                 no event matched {}",
                self.scope()
            );
        }

        let mut failures = Vec::with_capacity(events.len());
        for (position, event) in events.iter().enumerate() {
            let payload = event_data(event);
            let topics = event_topics(event).join(", ");
            let reason = match resolve_path(&payload, &path) {
                Ok(leaf) => match check(self.env, leaf) {
                    Ok(()) => return,
                    Err(reason) => reason,
                },
                Err(error) => error.describe(&path),
            };
            let tree = payload_tree(self.env, &payload);
            failures.push(format!(
                "  [{position}] topics [{topics}]\n    {reason}\n    payload tree:\n{}",
                indent_tree(&tree, "      "),
            ));
        }

        panic!(
            "assert_event_payload_field! failed: no event payload satisfied {expectation}\n\
             checked {} event(s) matching {}\n\n{}",
            events.len(),
            self.scope(),
            failures.join("\n\n"),
        );
    }

    /// The matcher that applies the configured filters, so the scope of this
    /// assertion is the scope of the event API it wraps.
    fn matcher(&self) -> EventMatcher<'a> {
        let mut matcher = EventMatcher::new(self.env);
        if let Some(contract) = &self.contract {
            matcher = matcher.from_contract(contract);
        }
        if let Some(topic) = &self.topic {
            matcher = matcher.with_topic(topic.clone());
        }
        matcher
    }

    fn configured_path(&self) -> String {
        match &self.path {
            Some(path) => path.clone(),
            None => panic!(
                "assert_event_payload_field! needs a `path:` entry, e.g. path: \"details.price\""
            ),
        }
    }

    /// The filters as a sentence, for the failure message.
    fn scope(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(topic) = &self.topic {
            parts.push(format!("topic {topic:?}"));
        }
        if let Some(contract) = &self.contract {
            parts.push(format!("contract {contract:?}"));
        }
        if parts.is_empty() {
            "no filter".to_string()
        } else {
            parts.join(" and ")
        }
    }
}

/// Asserts one field of an emitted event's payload, addressed by a dot path.
///
/// ```text
/// assert_event_payload_field!(&env, {
///     topic: "order",
///     path: "details.price",
///     equals: 500i128,
/// });
///
/// assert_event_payload_field!(&env, {
///     topic: "order",
///     path: "details.royalty_bps",
///     matches: |bps: u32| bps <= 2_500,
/// });
/// ```
///
/// Fields, all optional except the final `equals:` or `matches:`:
///
/// | Field | Meaning |
/// |-------|---------|
/// | `topic: "order"` | Only events carrying this topic symbol are searched |
/// | `contract: &id` | Only events published by this contract are searched |
/// | `path: "a.b.0"` | Map keys separated by dots; a numeric segment or `[0]` indexes a vector |
/// | `equals: value` | The leaf must equal `value`, converted to its type |
/// | `matches: closure` | The leaf must satisfy the closure, converted to the type its parameter names |
///
/// `equals:` and `matches:` are terminal: they end the block. Any of the
/// filters may be omitted, and may appear in any order before them.
///
/// A path that does not resolve is reported with the segment that stopped it,
/// the keys that were available there, and the full payload tree:
///
/// ```text
/// assert_event_payload_field! failed: no event payload satisfied "details.pricing" == 500
/// checked 1 event(s) matching topic "order"
///
///   [0] topics [order]
///     stopped at segment 2 ("pricing"): "details" has no key "pricing"; available keys: price, royalty_bps
///     payload tree:
///       {
///         "order_id": 42,
///         "details": {
///           "price": 500,
///           "royalty_bps": 250
///         }
///       }
/// ```
#[macro_export]
macro_rules! assert_event_payload_field {
    ($env:expr, { $($body:tt)* }) => {{
        $crate::__assert_event_payload_field!(
            $crate::payload::PayloadAssertion::new(&$env);
            $($body)*
        )
    }};
}

/// Parses the block of [`assert_event_payload_field!`](crate::assert_event_payload_field).
#[doc(hidden)]
#[macro_export]
macro_rules! __assert_event_payload_field {
    ($assertion:expr; equals: $expected:expr $(,)?) => {
        $assertion.assert_equals($expected)
    };
    ($assertion:expr; matches: $predicate:expr $(,)?) => {
        $assertion.assert_matches($predicate)
    };
    ($assertion:expr; topic: $topic:expr, $($rest:tt)+) => {
        $crate::__assert_event_payload_field!($assertion.with_topic($topic); $($rest)+)
    };
    ($assertion:expr; contract: $contract:expr, $($rest:tt)+) => {
        $crate::__assert_event_payload_field!($assertion.from_contract(&$contract); $($rest)+)
    };
    ($assertion:expr; path: $path:expr, $($rest:tt)+) => {
        $crate::__assert_event_payload_field!($assertion.path($path); $($rest)+)
    };
    ($assertion:expr; topic: $topic:expr $(,)?) => {
        compile_error!(
            "assert_event_payload_field! needs `equals:` or `matches:` after its filters"
        )
    };
    ($assertion:expr; contract: $contract:expr $(,)?) => {
        compile_error!(
            "assert_event_payload_field! needs `equals:` or `matches:` after its filters"
        )
    };
    ($assertion:expr; path: $path:expr $(,)?) => {
        compile_error!(
            "assert_event_payload_field! needs `equals:` or `matches:` after its path"
        )
    };
    ($assertion:expr;) => {
        compile_error!(
            "assert_event_payload_field! needs a `path:` and an `equals:` or `matches:` entry"
        )
    };
}

/// Reads a ledger value as `T`, or `None` when `T` does not describe it.
pub(crate) fn read_as<T>(env: &Env, value: &ScVal) -> Option<T>
where
    T: TryFromVal<Env, Val>,
{
    let val = Val::try_from_val(env, value).ok()?;
    T::try_from_val(env, &val).ok()
}

/// One step of a parsed path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment {
    name: Option<String>,
    index: Option<usize>,
}

impl Segment {
    /// The segment as the user wrote it, for diagnostics: `details`, `0`,
    /// `lots[2]`.
    fn label(&self) -> String {
        match (&self.name, self.index) {
            (Some(name), Some(index)) => format!("{name}[{index}]"),
            (Some(name), None) => name.clone(),
            (None, Some(index)) => index.to_string(),
            (None, None) => String::new(),
        }
    }
}

/// The step of a path navigation that could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathError {
    /// 1-based position of the failing segment in the path.
    segment: usize,
    /// The failing segment as written.
    segment_text: String,
    /// The prefix that did resolve, e.g. `details`; empty at the payload root.
    traversed: String,
    kind: PathFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathFailure {
    EmptyPath,
    EmptySegment,
    MalformedIndex,
    MissingField { available: Vec<String> },
    NotAMap { shape: &'static str },
    NotAVector { shape: &'static str },
    IndexOutOfBounds { len: usize },
}

impl PathError {
    fn describe(&self, path: &str) -> String {
        let location = if self.traversed.is_empty() {
            "the payload root".to_string()
        } else {
            format!("{:?}", self.traversed)
        };

        match &self.kind {
            PathFailure::EmptyPath => format!(
                "path {path:?} is empty; give a dot-separated path such as \"details.price\""
            ),
            PathFailure::EmptySegment => format!(
                "path {path:?} has an empty segment; write one field name per dot, such as \"details.price\""
            ),
            PathFailure::MalformedIndex => format!(
                "path {path:?} has a malformed index in segment {} ({:?}); write \"lots[0]\" or \"lots.0\"",
                self.segment, self.segment_text
            ),
            PathFailure::MissingField { available } => {
                let keys = if available.is_empty() {
                    "none".to_string()
                } else {
                    available.join(", ")
                };
                format!(
                    "stopped at segment {} ({:?}): {} has no key {:?}; available keys: {}",
                    self.segment, self.segment_text, location, self.segment_text, keys
                )
            }
            PathFailure::NotAMap { shape } => format!(
                "stopped at segment {} ({:?}): {} holds {}, which is not a map, so {:?} names no key",
                self.segment, self.segment_text, location, shape, self.segment_text
            ),
            PathFailure::NotAVector { shape } => format!(
                "stopped at segment {} ({:?}): {} holds {}, which is not a vector, so {:?} names no index",
                self.segment, self.segment_text, location, shape, self.segment_text
            ),
            PathFailure::IndexOutOfBounds { len } => format!(
                "stopped at segment {} ({:?}): {} has {} entries, so that index is out of bounds",
                self.segment, self.segment_text, location, len
            ),
        }
    }
}

/// Splits a path into the steps navigation takes.
fn parse_path(path: &str) -> Result<Vec<Segment>, PathError> {
    if path.trim().is_empty() {
        return Err(PathError {
            segment: 0,
            segment_text: String::new(),
            traversed: String::new(),
            kind: PathFailure::EmptyPath,
        });
    }

    let mut segments = Vec::new();
    for (position, raw) in path.split('.').enumerate() {
        let number = position + 1;
        let malformed = |kind: PathFailure| PathError {
            segment: number,
            segment_text: raw.to_string(),
            traversed: String::new(),
            kind,
        };

        if raw.is_empty() {
            return Err(malformed(PathFailure::EmptySegment));
        }

        if let Some(open) = raw.find('[') {
            if !raw.ends_with(']') {
                return Err(malformed(PathFailure::MalformedIndex));
            }
            let name = &raw[..open];
            let index = match raw[open + 1..raw.len() - 1].parse::<usize>() {
                Ok(index) => index,
                Err(_) => return Err(malformed(PathFailure::MalformedIndex)),
            };
            let name = if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            };
            segments.push(Segment {
                name,
                index: Some(index),
            });
        } else if raw.chars().all(|character| character.is_ascii_digit()) {
            match raw.parse::<usize>() {
                Ok(index) => segments.push(Segment {
                    name: None,
                    index: Some(index),
                }),
                Err(_) => return Err(malformed(PathFailure::MalformedIndex)),
            }
        } else {
            segments.push(Segment {
                name: Some(raw.to_string()),
                index: None,
            });
        }
    }

    Ok(segments)
}

/// Walks `path` down a payload, reporting the step that did not resolve.
fn resolve_path<'value>(root: &'value ScVal, path: &str) -> Result<&'value ScVal, PathError> {
    let segments = parse_path(path)?;
    let mut current = root;
    let mut traversed: Vec<String> = Vec::new();

    for (position, segment) in segments.iter().enumerate() {
        let number = position + 1;

        if let Some(name) = &segment.name {
            match current {
                ScVal::Map(Some(entries)) => {
                    let mut available = Vec::new();
                    let mut found = None;
                    for entry in entries.iter() {
                        if let Some(text) = map_key_text(&entry.key) {
                            if text == *name {
                                found = Some(&entry.val);
                                break;
                            }
                            available.push(text);
                        }
                    }
                    match found {
                        Some(value) => {
                            current = value;
                            traversed.push(name.clone());
                        }
                        None => {
                            return Err(path_error(
                                number,
                                name.clone(),
                                &traversed,
                                PathFailure::MissingField { available },
                            ))
                        }
                    }
                }
                // A void map resolves every key to nothing, so it fails the
                // same way an empty map does.
                ScVal::Map(None) => {
                    return Err(path_error(
                        number,
                        name.clone(),
                        &traversed,
                        PathFailure::MissingField {
                            available: Vec::new(),
                        },
                    ))
                }
                other => {
                    return Err(path_error(
                        number,
                        name.clone(),
                        &traversed,
                        PathFailure::NotAMap {
                            shape: shape_of(other),
                        },
                    ))
                }
            }
        }

        if let Some(index) = segment.index {
            match current {
                ScVal::Vec(Some(items)) => {
                    let len = items.len();
                    match items.get(index) {
                        Some(value) => {
                            current = value;
                            traversed.push(index.to_string());
                        }
                        None => {
                            return Err(path_error(
                                number,
                                segment.label(),
                                &traversed,
                                PathFailure::IndexOutOfBounds { len },
                            ))
                        }
                    }
                }
                other => {
                    return Err(path_error(
                        number,
                        segment.label(),
                        &traversed,
                        PathFailure::NotAVector {
                            shape: shape_of(other),
                        },
                    ))
                }
            }
        }
    }

    Ok(current)
}

fn path_error(
    segment: usize,
    segment_text: impl Into<String>,
    traversed: &[String],
    kind: PathFailure,
) -> PathError {
    PathError {
        segment,
        segment_text: segment_text.into(),
        traversed: traversed.join("."),
        kind,
    }
}

/// The text a map key carries, for the keys a struct is published under.
fn map_key_text(key: &ScVal) -> Option<String> {
    match key {
        ScVal::Symbol(symbol) => {
            Some(String::from_utf8_lossy(AsRef::<[u8]>::as_ref(symbol)).into_owned())
        }
        ScVal::String(text) => {
            Some(String::from_utf8_lossy(AsRef::<[u8]>::as_ref(text)).into_owned())
        }
        _ => None,
    }
}

/// The kind of value a path step ran into, for a diagnostic.
fn shape_of(value: &ScVal) -> &'static str {
    match value {
        ScVal::Map(_) => "map",
        ScVal::Vec(_) => "vector",
        ScVal::U32(_) => "u32",
        ScVal::I32(_) => "i32",
        ScVal::U64(_) => "u64",
        ScVal::I64(_) => "i64",
        ScVal::U128(_) => "u128",
        ScVal::I128(_) => "i128",
        ScVal::Bool(_) => "bool",
        ScVal::Symbol(_) => "symbol",
        ScVal::String(_) => "string",
        ScVal::Bytes(_) => "bytes",
        ScVal::Address(_) => "address",
        ScVal::Void => "void",
        _ => "non-container value",
    }
}

/// A whole payload rendered as an indented tree, one field or element per line.
fn payload_tree(env: &Env, value: &ScVal) -> String {
    let mut out = String::new();
    write_tree(env, value, 0, &mut out);
    out
}

fn write_tree(env: &Env, value: &ScVal, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);

    match value {
        ScVal::Map(Some(entries)) if !entries.is_empty() => {
            out.push_str("{\n");
            for (position, entry) in entries.iter().enumerate() {
                if position > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&"  ".repeat(depth + 1));
                match map_key_text(&entry.key) {
                    Some(key) => {
                        out.push('"');
                        out.push_str(&key);
                        out.push('"');
                    }
                    None => out.push_str(&data_label(env, &entry.key)),
                }
                out.push_str(": ");
                write_tree(env, &entry.val, depth + 1, out);
            }
            out.push('\n');
            out.push_str(&indent);
            out.push('}');
        }
        ScVal::Vec(Some(items)) if !items.is_empty() => {
            out.push_str("[\n");
            for (position, item) in items.iter().enumerate() {
                if position > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&"  ".repeat(depth + 1));
                write_tree(env, item, depth + 1, out);
            }
            out.push('\n');
            out.push_str(&indent);
            out.push(']');
        }
        leaf => out.push_str(&data_label(env, leaf)),
    }
}

/// Indents every line of a rendered tree so it sits under the failure it belongs to.
fn indent_tree(tree: &str, pad: &str) -> String {
    tree.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::PayloadAssertion;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, vec, Address, Env, Vec};

    #[contracttype]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Line {
        pub price: i128,
        pub royalty_bps: u32,
    }

    #[contracttype]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Order {
        pub order_id: u64,
        pub details: Line,
        pub lots: Vec<u32>,
    }

    #[contract]
    pub struct OrderContract;

    #[contractimpl]
    impl OrderContract {
        #[allow(deprecated)]
        pub fn place(env: Env, order_id: u64, price: i128, royalty_bps: u32) {
            env.events().publish(
                (symbol_short!("order"),),
                Order {
                    order_id,
                    details: Line { price, royalty_bps },
                    lots: vec![&env, 3u32, 7u32],
                },
            );
        }

        pub fn silent(env: Env) {
            let _ = env;
        }
    }

    fn placed() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(OrderContract, ());
        OrderContractClient::new(&env, &contract).place(&42u64, &500i128, &250u32);
        (env, contract)
    }

    #[test]
    fn reads_a_nested_struct_field() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.price",
            equals: 500i128,
        });
    }

    #[test]
    fn reads_a_root_level_field() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "order_id",
            equals: 42u64,
        });
    }

    #[test]
    fn navigates_into_a_vector_by_number_or_brackets() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "lots.1",
            equals: 7u32,
        });
        assert_event_payload_field!(&env, {
            topic: "order",
            path: "lots[0]",
            equals: 3u32,
        });
    }

    #[test]
    fn matches_a_predicate_over_the_navigated_field() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.royalty_bps",
            matches: |bps: u32| bps <= 2_500,
        });
    }

    #[test]
    fn filters_by_the_emitting_contract() {
        let (env, contract) = placed();

        assert_event_payload_field!(&env, {
            contract: &contract,
            topic: "order",
            path: "details.price",
            equals: 500i128,
        });
    }

    #[test]
    fn accepts_a_builder_with_the_same_filters() {
        let (env, _) = placed();

        PayloadAssertion::new(&env)
            .with_topic("order")
            .path("details.price")
            .assert_equals(500i128);
    }

    #[test]
    #[should_panic(expected = "has no key \"pricing\"")]
    fn reports_the_missing_intermediate_key() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.pricing.rate",
            equals: 1u32,
        });
    }

    #[test]
    #[should_panic(expected = "available keys: price, royalty_bps")]
    fn lists_the_keys_that_were_available() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.pricing",
            equals: 1u32,
        });
    }

    #[test]
    #[should_panic(expected = "payload tree:")]
    fn prints_the_payload_tree_on_failure() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.price",
            equals: 501i128,
        });
    }

    #[test]
    #[should_panic(expected = "resolved to 500, expected 501")]
    fn reports_the_value_it_found() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.price",
            equals: 501i128,
        });
    }

    #[test]
    #[should_panic(expected = "which does not convert to u32")]
    fn reports_a_type_conversion_mismatch() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details",
            equals: 5u32,
        });
    }

    #[test]
    #[should_panic(expected = "which the predicate rejected")]
    fn reports_a_rejected_predicate() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.price",
            matches: |price: i128| price > 10_000,
        });
    }

    #[test]
    #[should_panic(expected = "has 2 entries, so that index is out of bounds")]
    fn reports_an_index_past_the_end_of_a_vector() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "lots.5",
            equals: 3u32,
        });
    }

    #[test]
    #[should_panic(expected = "which is not a map")]
    fn reports_a_segment_that_needs_a_map() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.price.amount",
            equals: 3i128,
        });
    }

    #[test]
    #[should_panic(expected = "no event in scope")]
    fn reports_an_empty_scope() {
        let (env, _contract) = placed();
        let other = Address::generate(&env);

        assert_event_payload_field!(&env, {
            contract: &other,
            topic: "order",
            path: "details.price",
            equals: 500i128,
        });
    }

    #[test]
    #[should_panic(expected = "no event in scope")]
    fn reports_an_empty_env_when_the_topic_never_fired() {
        let env = Env::default();
        let contract = env.register(OrderContract, ());
        OrderContractClient::new(&env, &contract).silent();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details.price",
            equals: 500i128,
        });
    }

    #[test]
    #[should_panic(expected = "needs a `path:` entry")]
    fn reports_a_missing_path() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            equals: 500i128,
        });
    }

    #[test]
    #[should_panic(expected = "has an empty segment")]
    fn reports_a_malformed_path() {
        let (env, _) = placed();

        assert_event_payload_field!(&env, {
            topic: "order",
            path: "details..price",
            equals: 500i128,
        });
    }
}
