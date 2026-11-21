use soroban_sdk::testutils::Events as _;
use soroban_sdk::{Address, Env, TryFromVal};

/// Ergonomic assertions over the events a contract published.
///
/// By default a matcher reads `env.events().all()`, which in SDK v28 reports the
/// events of the **most recent contract invocation** only, so assert between
/// calls. To read several calls as one set, gather them into an [`EventLog`] and
/// take [`EventLog::matcher`] — the filters and every assertion below then apply
/// to the whole sequence.
pub struct EventMatcher<'a> {
    env: &'a Env,
    contract: Option<Address>,
    topics: Vec<String>,
    events: Option<&'a [soroban_sdk::xdr::ContractEvent]>,
}

impl<'a> EventMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            contract: None,
            topics: Vec::new(),
            events: None,
        }
    }

    pub fn env(&self) -> &'a Env {
        self.env
    }

    pub fn events_slice(&self) -> Option<&'a [soroban_sdk::xdr::ContractEvent]> {
        self.events
    }

    pub fn from_contract(mut self, contract: &Address) -> Self {
        self.contract = Some(contract.clone());
        self
    }

    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topics.push(topic.into());
        self
    }

    pub fn with_topics<I, S>(mut self, topics: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for topic in topics {
            self.topics.push(topic.into());
        }
        self
    }

    /// The events in scope after the contract filter only: what a count
    /// diagnostic prints so the author sees everything the call published, not
    /// just what survived the topic filter.
    fn scoped(&self) -> Vec<soroban_sdk::xdr::ContractEvent> {
        let mut events = match self.events {
            Some(collected) => collected.to_vec(),
            None => self.env.events().all().events().to_vec(),
        };

        if let Some(contract) = &self.contract {
            let wanted = emitting_contract(contract);
            events.retain(
                |event| match (wanted.as_ref(), event.contract_id.as_ref()) {
                    (Some(id), Some(emitter)) => id == emitter,
                    _ => false,
                },
            );
        }
        events
    }

    /// Applies the configured filters to the events in scope, whether that scope
    /// is the latest invocation or an [`EventLog`].
    pub(crate) fn matched(&self) -> Vec<soroban_sdk::xdr::ContractEvent> {
        let mut events = self.scoped();
        for topic in &self.topics {
            events.retain(|event| has_topic(event, topic));
        }
        events
    }

    /// Which events this matcher reads, in the words a failure message uses.
    fn scope_label(&self) -> String {
        match self.events {
            Some(collected) => format!("event log ({} collected)", collected.len()),
            None => "most recent invocation".to_string(),
        }
    }

    pub fn assert_emitted(&self) {
        assert!(
            !self.matched().is_empty(),
            "Expected at least one event to be emitted"
        );
    }

    pub fn assert_count(&self, expected: usize) {
        let count = self.matched().len();
        assert_eq!(
            count, expected,
            "Expected {} events, found {}",
            expected, count
        );
    }

    /// Fail when nothing in scope was published.
    ///
    /// Honours [`Self::from_contract`] and [`Self::with_topic`], so
    /// `with_topic("Transfer").assert_not_emitted()` reads as "no Transfer event
    /// came out of that call".
    ///
    /// Scope is the most recent contract invocation for a matcher built by
    /// [`EventMatcher::new`], and the whole collected sequence for a matcher from
    /// [`EventLog::matcher`] — which is how a test proves an event never appeared
    /// at all, rather than only in the last call.
    pub fn assert_not_emitted(&self) {
        let found = self.matched();
        if let Some(first) = found.first() {
            panic!(
                "Expected no events to be emitted, found {} — unexpected event: {}",
                found.len(),
                describe(first),
            );
        }
    }

    /// Fail when any event in scope satisfies `predicate`.
    ///
    /// Use it for what the topic filter cannot express: the raw
    /// [`ContractEvent`](soroban_sdk::xdr::ContractEvent) is handed over, so a
    /// predicate can read the event type or inspect its data payload.
    pub fn assert_none_match(&self, predicate: impl Fn(&soroban_sdk::xdr::ContractEvent) -> bool) {
        if let Some(offending) = self.matched().iter().find(|event| predicate(event)) {
            panic!(
                "Expected no event to match, found one: {}",
                describe(offending)
            );
        }
    }

    /// Fail when no event in scope carries a data payload that satisfies `predicate`.
    ///
    /// The predicate receives an [`EventData`] view of the payload, so a test can
    /// deserialize it into a type or read one field of it. The filters apply as
    /// usual, and pairing this with [`Self::with_topic`] is how you name the event
    /// whose fields you mean: `with_topic("transfer")` followed by a predicate
    /// `|data| data.deserialize::<u32>() == Some(1_000)`.
    ///
    /// See the [events guide](https://stellar-crucible.github.io/soroban-testkit/crates/assert.html)
    /// for the worked examples.
    pub fn assert_data_matches(&self, predicate: impl Fn(&EventData<'_>) -> bool) {
        let events = self.matched();
        if events.is_empty() {
            panic!("Expected an event whose data matches, but no event was emitted");
        }

        let mut seen = Vec::with_capacity(events.len());
        for event in &events {
            let data = EventData::new(self.env, event_data(event));
            if predicate(&data) {
                return;
            }
            seen.push(data_label(self.env, data.raw()));
        }

        panic!(
            "Expected an event whose data matches, checked {} event(s) with data [{}]",
            events.len(),
            seen.join(", ")
        );
    }

    /// Assert that every matching event has exactly the named fields and that
    /// each field can be deserialized as its declared type.
    pub fn assert_data_schema(&self, schema: &[EventFieldSchema]) {
        let events = self.matched();
        if events.is_empty() {
            let topic_desc = if self.topics.is_empty() {
                "<any>".to_string()
            } else {
                self.topics.join(", ")
            };
            panic!(
                "Expected an event matching topic {:?} for schema validation, but no event was emitted",
                topic_desc
            );
        }

        let mut failures = Vec::new();
        for (index, event) in events.iter().enumerate() {
            let data = EventData::new(self.env, event_data(event));
            let errors = schema_errors(&data, schema);
            if !errors.is_empty() {
                failures.push(format!("event {}: {}", index + 1, errors.join("; ")));
            }
        }

        if !failures.is_empty() {
            let topic_desc = if self.topics.is_empty() {
                "<any>".to_string()
            } else {
                self.topics.join(", ")
            };
            panic!(
                "Event data schema mismatch for topic {:?}:\n{}",
                topic_desc,
                failures.join("\n")
            );
        }
    }

    /// Fail unless the number of matching events satisfies `bound`.
    ///
    /// `bound` is an exact count (`3`) or any range: `3..=5`, `2..`, `..=4`,
    /// `1..10`. Use a range when a call may add auxiliary events (fees, logs,
    /// hooks) and an exact count would break each time one is added.
    ///
    /// Honours [`Self::from_contract`] and [`Self::with_topic`]. On failure the
    /// message carries a table of every topic signature in scope and how often
    /// it appeared, so the author can see what was emitted instead.
    pub fn assert_count_within(&self, bound: impl EventCountBound) {
        let count = self.matched().len();
        if !bound.contains_count(count) {
            let what = if self.topics.is_empty() {
                "event(s)".to_string()
            } else {
                format!("event(s) with topic \"{}\"", self.topics.join(", "))
            };
            panic!(
                "Event count out of range: expected {} {what}, observed {count}\n{}",
                bound.describe(),
                self.observed_table(),
            );
        }
    }

    /// Fail unless the number of events carrying the `topic` symbol satisfies
    /// `bound`.
    ///
    /// An event counts when any of its topics is the symbol `topic`, the same
    /// rule [`Self::with_topic`] applies. `bound` is an exact frequency (`2`) or
    /// a range (`1..=3`). The filters already configured on the matcher apply
    /// first, so `from_contract(&id).assert_topic_count("transfer", 2)` counts
    /// only that contract's transfers.
    ///
    /// On failure the message names the topic, says whether it was seen at all,
    /// and prints the table of every topic signature in scope.
    pub fn assert_topic_count(&self, topic: impl Into<String>, bound: impl EventCountBound) {
        let topic = topic.into();
        let count = self
            .matched()
            .iter()
            .filter(|event| has_topic(event, &topic))
            .count();
        if !bound.contains_count(count) {
            let note = if count == 0 {
                format!(" (topic \"{topic}\" was not observed)")
            } else {
                String::new()
            };
            panic!(
                "Event topic count mismatch: expected topic \"{topic}\" {} time(s), observed {count}{note}\n{}",
                bound.describe(),
                self.observed_table(),
            );
        }
    }

    /// A table of the topic signatures in scope (after the contract filter) and
    /// how many events carried each, in the order they first appeared.
    fn observed_table(&self) -> String {
        let events = self.scoped();
        let mut rows: Vec<(String, usize)> = Vec::new();
        for event in &events {
            let signature = topic_signature(event);
            match rows.iter_mut().find(|(seen, _)| *seen == signature) {
                Some((_, count)) => *count += 1,
                None => rows.push((signature, 1)),
            }
        }
        format_topic_table(&self.scope_label(), &rows, events.len())
    }
}

/// How many events an assertion accepts: an exact count or a range of counts.
///
/// Implemented for the integer types and every standard range over them, so a
/// test writes `3`, `3..=5`, `3..`, `..5`, `..=5`, `1..10`, or `..`. Integer
/// literals need no suffix. A negative bound simply matches no count.
pub trait EventCountBound {
    /// Whether `count` events satisfy the bound.
    fn contains_count(&self, count: usize) -> bool;

    /// The bound as a failure message spells it, e.g. `exactly 2` or `3..=5`.
    fn describe(&self) -> String;
}

macro_rules! impl_event_count_bound {
    ($($int:ty),* $(,)?) => {$(
        impl EventCountBound for $int {
            fn contains_count(&self, count: usize) -> bool {
                count as i128 == *self as i128
            }
            fn describe(&self) -> String {
                format!("exactly {}", self)
            }
        }

        impl EventCountBound for ::core::ops::Range<$int> {
            fn contains_count(&self, count: usize) -> bool {
                let count = count as i128;
                (self.start as i128) <= count && count < (self.end as i128)
            }
            fn describe(&self) -> String {
                format!("{}..{}", self.start, self.end)
            }
        }

        impl EventCountBound for ::core::ops::RangeInclusive<$int> {
            fn contains_count(&self, count: usize) -> bool {
                let count = count as i128;
                (*self.start() as i128) <= count && count <= (*self.end() as i128)
            }
            fn describe(&self) -> String {
                format!("{}..={}", self.start(), self.end())
            }
        }

        impl EventCountBound for ::core::ops::RangeFrom<$int> {
            fn contains_count(&self, count: usize) -> bool {
                (self.start as i128) <= count as i128
            }
            fn describe(&self) -> String {
                format!("{}..", self.start)
            }
        }

        impl EventCountBound for ::core::ops::RangeTo<$int> {
            fn contains_count(&self, count: usize) -> bool {
                (count as i128) < (self.end as i128)
            }
            fn describe(&self) -> String {
                format!("..{}", self.end)
            }
        }

        impl EventCountBound for ::core::ops::RangeToInclusive<$int> {
            fn contains_count(&self, count: usize) -> bool {
                (count as i128) <= (self.end as i128)
            }
            fn describe(&self) -> String {
                format!("..={}", self.end)
            }
        }
    )*};
}

impl_event_count_bound!(usize, u8, u16, u32, u64, i32, i64);

impl EventCountBound for ::core::ops::RangeFull {
    fn contains_count(&self, _count: usize) -> bool {
        true
    }
    fn describe(&self) -> String {
        "any number (..)".to_string()
    }
}

/// Anything the event-count macros can read events from: an [`Env`] (the most
/// recent invocation) or an [`EventLog`] (every invocation it collected).
///
/// References to either work too, so `env`, `&env`, `log`, and `&log` are all
/// accepted by [`assert_event_count_range!`](crate::assert_event_count_range)
/// and [`assert_event_topic_count!`](crate::assert_event_topic_count).
pub trait EventSource {
    /// A fresh matcher over this source, with no filters applied.
    fn event_matcher(&self) -> EventMatcher<'_>;
}

impl EventSource for Env {
    fn event_matcher(&self) -> EventMatcher<'_> {
        EventMatcher::new(self)
    }
}

impl EventSource for EventLog<'_> {
    fn event_matcher(&self) -> EventMatcher<'_> {
        self.matcher()
    }
}

impl<T: EventSource + ?Sized> EventSource for &T {
    fn event_matcher(&self) -> EventMatcher<'_> {
        (**self).event_matcher()
    }
}

impl<T: EventSource + ?Sized> EventSource for &mut T {
    fn event_matcher(&self) -> EventMatcher<'_> {
        (**self).event_matcher()
    }
}

/// Assert that the number of events in scope falls within a range.
///
/// The first argument is an [`Env`](soroban_sdk::Env) (the most recent
/// invocation) or an [`EventLog`](crate::events::EventLog) (every collected
/// invocation). The bound is an exact count or any range (`3..=5`, `2..`,
/// `..=4`). Add `from = contract` to count only one contract's events.
///
/// On failure the panic message prints a table of every observed topic
/// signature and how many events carried it.
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_event_count_range;
///
/// client.batch_transfer(&from, &recipients);
/// assert_event_count_range!(env, 3..=5);
/// assert_event_count_range!(env, from = contract_id, 3..);
/// assert_event_count_range!(log, ..=10);
/// ```
#[macro_export]
macro_rules! assert_event_count_range {
    ($source:expr, from = $contract:expr, $bound:expr $(,)?) => {{
        $crate::events::EventSource::event_matcher(&$source)
            .from_contract(&$contract)
            .assert_count_within($bound)
    }};
    ($source:expr, $bound:expr $(,)?) => {{
        $crate::events::EventSource::event_matcher(&$source).assert_count_within($bound)
    }};
}

/// Assert how many events in scope carry a topic symbol.
///
/// The first argument is an [`Env`](soroban_sdk::Env) or an
/// [`EventLog`](crate::events::EventLog). The count is exact (`2`) or any range
/// (`1..=3`). Add `from = contract` to count only one contract's events.
///
/// On failure the panic message says whether the topic was observed at all and
/// prints a table of every observed topic signature with its count.
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_event_topic_count;
///
/// assert_event_topic_count!(env, "transfer", 2);
/// assert_event_topic_count!(env, "fee", 0..=1);
/// assert_event_topic_count!(log, from = token_id, "mint", 3);
/// ```
#[macro_export]
macro_rules! assert_event_topic_count {
    ($source:expr, from = $contract:expr, $topic:expr, $bound:expr $(,)?) => {{
        $crate::events::EventSource::event_matcher(&$source)
            .from_contract(&$contract)
            .assert_topic_count($topic, $bound)
    }};
    ($source:expr, $topic:expr, $bound:expr $(,)?) => {{
        $crate::events::EventSource::event_matcher(&$source).assert_topic_count($topic, $bound)
    }};
}

/// Whether any topic of `event` is the symbol `topic`.
pub(crate) fn has_topic(event: &soroban_sdk::xdr::ContractEvent, topic: &str) -> bool {
    let bytes = topic.as_bytes();
    let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
    body.topics.iter().any(|topic| match topic {
        soroban_sdk::xdr::ScVal::Symbol(symbol) => AsRef::<[u8]>::as_ref(symbol) == bytes,
        _ => false,
    })
}

/// The shape of an event's topics, used to group rows in the count table.
///
/// Symbols are spelled out because they name the event. Every other topic
/// becomes its type in angle brackets, so transfers to different addresses
/// share one row instead of each getting its own.
fn topic_signature(event: &soroban_sdk::xdr::ContractEvent) -> String {
    let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
    if body.topics.is_empty() {
        return "(no topics)".to_string();
    }
    body.topics
        .iter()
        .map(|topic| match topic {
            soroban_sdk::xdr::ScVal::Symbol(symbol) => {
                String::from_utf8_lossy(AsRef::<[u8]>::as_ref(symbol)).into_owned()
            }
            other => format!("<{}>", other.name().to_ascii_lowercase()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders the observed-topics table a count assertion prints on failure.
fn format_topic_table(scope: &str, rows: &[(String, usize)], total: usize) -> String {
    const HEADER: &str = "Observed topics";
    let width = rows
        .iter()
        .map(|(signature, _)| signature.chars().count())
        .chain([HEADER.len(), "(none)".len(), "total".len()])
        .max()
        .unwrap_or(HEADER.len());

    let mut out = format!("Scope: {scope}\n");
    out.push_str(&format!("| {HEADER:<width$} | Count |\n"));
    out.push_str(&format!("|{}|-------|\n", "-".repeat(width + 2)));
    if rows.is_empty() {
        out.push_str(&format!("| {:<width$} | {:>5} |\n", "(none)", 0));
    }
    for (signature, count) in rows {
        out.push_str(&format!("| {signature:<width$} | {count:>5} |\n"));
    }
    out.push_str(&format!("| {:<width$} | {total:>5} |\n", "total"));
    out
}

/// One field and its expected deserialization type in an event schema.
pub struct EventFieldSchema {
    name: &'static str,
    type_name: &'static str,
    matches: fn(&EventData<'_>) -> bool,
}

impl EventFieldSchema {
    /// Creates a field descriptor. Prefer constructing these with
    /// [`assert_event_data_schema!`].
    pub fn new(
        name: &'static str,
        type_name: &'static str,
        matches: fn(&EventData<'_>) -> bool,
    ) -> Self {
        Self {
            name,
            type_name,
            matches,
        }
    }
}

/// Assert that a topic's event payload has the specified fields and types.
///
/// Every emitted event matching the topic is checked. Extra fields are rejected
/// as well, so the assertion detects both type drift and schema changes.
///
/// ```rust,ignore
/// # use soroban_testkit_assert::assert_event_data_schema;
/// # use soroban_sdk::{Address, Env};
/// # let env = Env::default();
/// assert_event_data_schema!(&env, "transfer", {
///     amount: i128,
///     recipient: Address,
/// });
/// ```
#[macro_export]
macro_rules! assert_event_data_schema {
    ($env:expr, $topic:expr, { $($field:ident : $ty:ty),* $(,)? }) => {{
        let __schema_env = $env;
        let __schema_topic = $topic;
        let __schema = [
            $(
                $crate::events::EventFieldSchema::new(
                    stringify!($field),
                    stringify!($ty),
                    |__data| __data.deserialize::<$ty>().is_some(),
                )
            ),*
        ];
        $crate::events::EventMatcher::new(__schema_env)
            .with_topic(__schema_topic)
            .assert_data_schema(&__schema)
    }};
}

#[doc(hidden)]
pub fn __as_env(env: &Env) -> &Env {
    env
}

#[doc(hidden)]
pub fn __as_contract(contract: &Address) -> &Address {
    contract
}

/// A human-readable line for an emitted event in assertion failure messages.
pub(crate) fn format_event(
    env: &Env,
    index: usize,
    event: &soroban_sdk::xdr::ContractEvent,
) -> String {
    let contract_str = match &event.contract_id {
        Some(id) => {
            let sc_addr = soroban_sdk::xdr::ScAddress::Contract(id.clone());
            Address::try_from_val(env, &soroban_sdk::xdr::ScVal::Address(sc_addr))
                .map(|a| format!("{a:?}"))
                .unwrap_or_else(|_| format!("{id:?}"))
        }
        None => "<none>".to_string(),
    };
    let topics_str = event_topics(event).join(", ");
    let data_str = data_label(env, &event_data(event));
    format!(
        "  [{}] contract: {}, topics: [{}], data: {}",
        index + 1,
        contract_str,
        topics_str,
        data_str
    )
}

#[doc(hidden)]
#[allow(clippy::type_complexity)]
pub fn assert_event_emitted_check(
    env: &Env,
    contract: Option<&Address>,
    topics: &[&str],
    data_matcher: Option<(&dyn Fn(&EventData<'_>) -> bool, &str)>,
) {
    let all_events = env.events().all().events().to_vec();
    let wanted_contract = contract.and_then(emitting_contract);

    let found = all_events.iter().any(|event| {
        if let Some(wanted_id) = &wanted_contract {
            match &event.contract_id {
                Some(emitter_id) if wanted_id == emitter_id => {}
                _ => return false,
            }
        }

        let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
        for &expected_topic in topics {
            let bytes = expected_topic.as_bytes();
            let matches_topic = body.topics.iter().any(|t| match t {
                soroban_sdk::xdr::ScVal::Symbol(sym) => AsRef::<[u8]>::as_ref(sym) == bytes,
                other => topic_label(other) == expected_topic,
            });
            if !matches_topic {
                return false;
            }
        }

        if let Some((data_fn, _)) = data_matcher {
            let event_data = EventData::new(env, event_data(event));
            if !data_fn(&event_data) {
                return false;
            }
        }

        true
    });

    if !found {
        let contract_desc = match contract {
            Some(addr) => format!("{addr:?}"),
            None => "<any>".to_string(),
        };
        let topics_desc = if topics.is_empty() {
            "<any>".to_string()
        } else {
            format!("[{}]", topics.join(", "))
        };
        let data_desc = match data_matcher {
            Some((_, desc)) => desc,
            None => "<any>",
        };

        let events_debug = if all_events.is_empty() {
            "  (no events emitted in the most recent invocation)".to_string()
        } else {
            all_events
                .iter()
                .enumerate()
                .map(|(i, e)| format_event(env, i, e))
                .collect::<Vec<_>>()
                .join("\n")
        };

        panic!(
            "assert_event_emitted! failed: no matching event found.\n\
             Expected event:\n\
               contract: {}\n\
               topic(s): {}\n\
               data: {}\n\
             Emitted events ({} in scope):\n\
             {}",
            contract_desc,
            topics_desc,
            data_desc,
            all_events.len(),
            events_debug
        );
    }
}

/// Assert that a matching event was emitted during the most recent contract invocation.
///
/// Supports filtering by emitting contract address, topic(s), and data fields or payload.
/// Panics with a clear diagnostic message including all emitted events if no matching
/// event is found. If multiple matching events are emitted, the assertion succeeds.
///
/// # Examples
///
/// Assert by contract, topic, and struct/map fields:
/// ```rust,ignore
/// assert_event_emitted!(env, contract_id, "Transfer", {
///     from: sender,
///     to: receiver,
///     amount: 1000i128,
/// });
/// ```
///
/// Assert with multiple topics:
/// ```rust,ignore
/// assert_event_emitted!(env, contract_id, ["Transfer", "Audit"], {
///     amount: 1000i128,
/// });
/// ```
///
/// Assert by contract and topic only:
/// ```rust,ignore
/// assert_event_emitted!(env, contract_id, "ping");
/// ```
#[macro_export]
macro_rules! assert_event_emitted {
    // 1. (env, contract, [topics], { fields })
    ($env:expr, $contract:expr, [ $($topic:expr),* $(,)? ], { $($field:ident : $val:expr),* $(,)? }) => {{
        let __env = $crate::events::__as_env(&$env);
        let __contract = $crate::events::__as_contract(&$contract);
        let __topics: &[&str] = &[ $( AsRef::<str>::as_ref(&$topic) ),* ];
        let __data_matcher = |__data: &$crate::events::EventData<'_>| -> bool {
            $(
                if let Some(__field_data) = __data.field(stringify!($field)) {
                    if __field_data.deserialize().as_ref() != Some(&$val) {
                        return false;
                    }
                } else {
                    return false;
                }
            )*
            true
        };
        let __data_desc = concat!("{ ", $( stringify!($field), ": ", stringify!($val), ", ", )* "}");
        $crate::events::assert_event_emitted_check(
            __env,
            Some(__contract),
            __topics,
            Some((&__data_matcher, __data_desc)),
        );
    }};

    // 2. (env, contract, [topics])
    ($env:expr, $contract:expr, [ $($topic:expr),* $(,)? ]) => {{
        let __env = $crate::events::__as_env(&$env);
        let __contract = $crate::events::__as_contract(&$contract);
        let __topics: &[&str] = &[ $( AsRef::<str>::as_ref(&$topic) ),* ];
        $crate::events::assert_event_emitted_check(
            __env,
            Some(__contract),
            __topics,
            None,
        );
    }};

    // 3. (env, contract, topic, { fields })
    ($env:expr, $contract:expr, $topic:expr, { $($field:ident : $val:expr),* $(,)? }) => {{
        let __env = $crate::events::__as_env(&$env);
        let __contract = $crate::events::__as_contract(&$contract);
        let __topic: &str = AsRef::<str>::as_ref(&$topic);
        let __topics = [__topic];
        let __data_matcher = |__data: &$crate::events::EventData<'_>| -> bool {
            $(
                if let Some(__field_data) = __data.field(stringify!($field)) {
                    if __field_data.deserialize().as_ref() != Some(&$val) {
                        return false;
                    }
                } else {
                    return false;
                }
            )*
            true
        };
        let __data_desc = concat!("{ ", $( stringify!($field), ": ", stringify!($val), ", ", )* "}");
        $crate::events::assert_event_emitted_check(
            __env,
            Some(__contract),
            &__topics,
            Some((&__data_matcher, __data_desc)),
        );
    }};

    // 4. (env, [topics], { fields })
    ($env:expr, [ $($topic:expr),* $(,)? ], { $($field:ident : $val:expr),* $(,)? }) => {{
        let __env = $crate::events::__as_env(&$env);
        let __topics: &[&str] = &[ $( AsRef::<str>::as_ref(&$topic) ),* ];
        let __data_matcher = |__data: &$crate::events::EventData<'_>| -> bool {
            $(
                if let Some(__field_data) = __data.field(stringify!($field)) {
                    if __field_data.deserialize().as_ref() != Some(&$val) {
                        return false;
                    }
                } else {
                    return false;
                }
            )*
            true
        };
        let __data_desc = concat!("{ ", $( stringify!($field), ": ", stringify!($val), ", ", )* "}");
        $crate::events::assert_event_emitted_check(
            __env,
            None,
            __topics,
            Some((&__data_matcher, __data_desc)),
        );
    }};

    // 5. (env, topic, { fields })
    ($env:expr, $topic:expr, { $($field:ident : $val:expr),* $(,)? }) => {{
        let __env = $crate::events::__as_env(&$env);
        let __topic: &str = AsRef::<str>::as_ref(&$topic);
        let __topics = [__topic];
        let __data_matcher = |__data: &$crate::events::EventData<'_>| -> bool {
            $(
                if let Some(__field_data) = __data.field(stringify!($field)) {
                    if __field_data.deserialize().as_ref() != Some(&$val) {
                        return false;
                    }
                } else {
                    return false;
                }
            )*
            true
        };
        let __data_desc = concat!("{ ", $( stringify!($field), ": ", stringify!($val), ", ", )* "}");
        $crate::events::assert_event_emitted_check(
            __env,
            None,
            &__topics,
            Some((&__data_matcher, __data_desc)),
        );
    }};

    // 6. (env, contract, topic, scalar/expression data)
    ($env:expr, $contract:expr, $topic:expr, $data:expr) => {{
        let __env = $crate::events::__as_env(&$env);
        let __contract = $crate::events::__as_contract(&$contract);
        let __topic: &str = AsRef::<str>::as_ref(&$topic);
        let __topics = [__topic];
        let __data_val = &$data;
        let __data_matcher = |__data: &$crate::events::EventData<'_>| -> bool {
            __data.deserialize().as_ref() == Some(__data_val)
        };
        let __data_desc = stringify!($data);
        $crate::events::assert_event_emitted_check(
            __env,
            Some(__contract),
            &__topics,
            Some((&__data_matcher, __data_desc)),
        );
    }};

    // 7. (env, contract, topic)
    ($env:expr, $contract:expr, $topic:expr) => {{
        let __env = $crate::events::__as_env(&$env);
        let __contract = $crate::events::__as_contract(&$contract);
        let __topic: &str = AsRef::<str>::as_ref(&$topic);
        let __topics = [__topic];
        $crate::events::assert_event_emitted_check(
            __env,
            Some(__contract),
            &__topics,
            None,
        );
    }};

    // 8. (env, [topics])
    ($env:expr, [ $($topic:expr),* $(,)? ]) => {{
        let __env = $crate::events::__as_env(&$env);
        let __topics: &[&str] = &[ $( AsRef::<str>::as_ref(&$topic) ),* ];
        $crate::events::assert_event_emitted_check(
            __env,
            None,
            __topics,
            None,
        );
    }};

    // 9. (env, topic)
    ($env:expr, $topic:expr) => {{
        let __env = $crate::events::__as_env(&$env);
        let __topic: &str = AsRef::<str>::as_ref(&$topic);
        let __topics = [__topic];
        $crate::events::assert_event_emitted_check(
            __env,
            None,
            &__topics,
            None,
        );
    }};
}

/// The events of several invocations, asserted over as one set.
///
/// `env.events().all()` reports only the most recent contract invocation, so a
/// matcher built by `EventMatcher::new` can never say "these three calls emitted
/// exactly two `Transfer` events". An `EventLog` collects after each call the
/// test cares about, and [`Self::matcher`] hands the accumulated set to the same
/// filters and assertions:
///
/// ```text
/// let mut log = EventLog::new(&env);
/// client.transfer(&from, &to, &100);
/// log.collect();
/// client.close_offer(&from);
/// log.collect();
///
/// log.matcher().with_topic("transfer").assert_count(1);
/// log.matcher().with_topic("refund").assert_not_emitted();
/// ```
///
/// Collection is explicit because the SDK offers no per-invocation hook: a call
/// nobody collected from contributes nothing to the log. Events stay in call
/// order — one contiguous run per collected invocation, in the order the contract
/// published them — so [`Self::topics`] reads the sequence back.
pub struct EventLog<'a> {
    env: &'a Env,
    events: Vec<soroban_sdk::xdr::ContractEvent>,
}

impl<'a> EventLog<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            events: Vec::new(),
        }
    }

    /// Appends the events of the invocation that has just run.
    ///
    /// An invocation that emitted nothing appends nothing, which is what makes an
    /// assertion over the log mean "not in any of these calls".
    pub fn collect(&mut self) -> &mut Self {
        self.events
            .extend(self.env.events().all().events().to_vec());
        self
    }

    /// A matcher over everything collected so far.
    ///
    /// Borrowing the log is what makes the matcher read the sequence rather than
    /// the last call: the filters and assertions below work unchanged over it.
    pub fn matcher(&self) -> EventMatcher<'_> {
        EventMatcher {
            env: self.env,
            contract: None,
            topics: Vec::new(),
            events: Some(&self.events),
        }
    }

    /// The topic symbols of each collected event, in collection order.
    ///
    /// This is the shape a sequence assertion reads: one entry per event, the
    /// topics of that event as the contract published them.
    pub fn topics(&self) -> Vec<Vec<String>> {
        self.events.iter().map(event_topics).collect()
    }

    /// The collected events themselves, for what the filters cannot express.
    pub fn events(&self) -> &[soroban_sdk::xdr::ContractEvent] {
        &self.events
    }

    pub fn env(&self) -> &'a Env {
        self.env
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The contract id a filter compares against.
///
/// Only a contract can publish an event, so the filter needs a contract id, and
/// an account address names no emitter: it yields `None` and matches nothing.
/// The SDK's own `ContractEvents::filter_by_contract` panics on an account
/// address instead, and works only on the events `env.events().all()` handed it
/// — an accumulated set needs the comparison made here.
pub(crate) fn emitting_contract(address: &Address) -> Option<soroban_sdk::xdr::ContractId> {
    let soroban_sdk::xdr::ScVal::Address(emitter) = soroban_sdk::xdr::ScVal::from(address) else {
        unreachable!("an Address always converts to ScVal::Address");
    };
    match emitter {
        soroban_sdk::xdr::ScAddress::Contract(id) => Some(id),
        _ => None,
    }
}

/// The topic symbols of one event, as readable text.
pub(crate) fn event_topics(event: &soroban_sdk::xdr::ContractEvent) -> Vec<String> {
    let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
    body.topics.iter().map(topic_label).collect()
}

/// One event's data payload, in the form the ledger stored it and in the form a
/// test names.
///
/// Both views exist because a payload can be a single number, a tuple, or the
/// map a struct published through `contractevent` turns into, and which one a
/// test is asserting on is a property of the contract, not of this type.
pub struct EventData<'a> {
    env: &'a Env,
    raw: soroban_sdk::xdr::ScVal,
}

impl<'a> EventData<'a> {
    pub(crate) fn new(env: &'a Env, raw: soroban_sdk::xdr::ScVal) -> Self {
        Self { env, raw }
    }

    /// The payload exactly as the contract published it.
    pub fn raw(&self) -> &soroban_sdk::xdr::ScVal {
        &self.raw
    }

    /// The payload as `T`, or `None` when `T` does not describe it.
    ///
    /// `T` may be a primitive, an `Address`, a `Vec`/`Map`, or a struct declared
    /// with [`contracttype`](soroban_sdk::contracttype) — anything convertible
    /// from a ledger value. A mismatch is `None` rather than a panic, so one
    /// predicate can probe a payload without assuming its shape.
    pub fn deserialize<T>(&self) -> Option<T>
    where
        T: soroban_sdk::TryFromVal<Env, soroban_sdk::Val>,
    {
        let val = soroban_sdk::Val::try_from_val(self.env, &self.raw).ok()?;
        T::try_from_val(self.env, &val).ok()
    }

    /// The value stored under `key` in a map-shaped payload.
    ///
    /// A struct published with `contractevent` arrives as a map keyed by field
    /// name, which makes this the way to read one field of a typed event without
    /// declaring a matching Rust type. `None` when the payload is not a map or
    /// has no such key.
    pub fn field(&self, key: &str) -> Option<EventData<'a>> {
        let soroban_sdk::xdr::ScVal::Map(Some(map)) = &self.raw else {
            return None;
        };
        let key = key.as_bytes();
        map.iter().find_map(|entry| {
            let soroban_sdk::xdr::ScVal::Symbol(symbol) = &entry.key else {
                return None;
            };
            (AsRef::<[u8]>::as_ref(symbol) == key)
                .then(|| EventData::new(self.env, entry.val.clone()))
        })
    }

    /// Enumerate the symbol field names present in a map-shaped payload, in ledger order.
    ///
    /// Returns a `Vec<String>` containing the keys of the map.
    /// Returns an empty vector when the payload is a scalar, a vector, or otherwise not a map.
    ///
    /// # Note
    ///
    /// The Soroban SDK publishes event maps **sparse** by default: a field whose
    /// value is `None` is absent from the map rather than stored as `void`.
    /// The returned fields therefore describe the fields the ledger actually carries,
    /// which may not include every field declared on the struct.
    pub fn fields(&self) -> Vec<String> {
        let soroban_sdk::xdr::ScVal::Map(Some(map)) = &self.raw else {
            return Vec::new();
        };
        map.iter()
            .filter_map(|entry| {
                let soroban_sdk::xdr::ScVal::Symbol(symbol) = &entry.key else {
                    return None;
                };
                let bytes: &[u8] = AsRef::<[u8]>::as_ref(symbol);
                String::from_utf8(bytes.to_vec()).ok()
            })
            .collect()
    }
}

fn schema_errors(data: &EventData<'_>, schema: &[EventFieldSchema]) -> Vec<String> {
    use soroban_sdk::xdr::ScVal;

    let ScVal::Map(Some(entries)) = &data.raw else {
        return vec![format!(
            "expected a map payload, found {} ({})",
            scval_type_name(&data.raw),
            data_label(data.env, &data.raw)
        )];
    };

    let mut errors = Vec::new();
    for field in schema {
        let entry = entries.iter().find(|entry| {
            let ScVal::Symbol(key) = &entry.key else {
                return false;
            };
            AsRef::<[u8]>::as_ref(key) == field.name.as_bytes()
        });

        match entry {
            None => errors.push(format!(
                "missing field `{}` (expected {})",
                field.name, field.type_name
            )),
            Some(entry) => {
                let value = EventData::new(data.env, entry.val.clone());
                if !(field.matches)(&value) {
                    errors.push(format!(
                        "field `{}` expected {}, found {} (value {})",
                        field.name,
                        field.type_name,
                        scval_type_name(&entry.val),
                        data_label(data.env, &entry.val)
                    ));
                }
            }
        }
    }

    for entry in entries.iter() {
        match &entry.key {
            ScVal::Symbol(symbol) => {
                let key = String::from_utf8_lossy(AsRef::<[u8]>::as_ref(&symbol));
                if !schema.iter().any(|field| field.name == key) {
                    errors.push(format!("unexpected field `{key}`"));
                }
            }
            key => errors.push(format!(
                "unexpected non-symbol map key {}",
                data_label(data.env, key)
            )),
        }
    }

    errors
}

fn scval_type_name(value: &soroban_sdk::xdr::ScVal) -> &'static str {
    use soroban_sdk::xdr::ScVal;

    match value {
        ScVal::Bool(_) => "bool",
        ScVal::I128(_) => "i128",
        ScVal::Address(_) => "Address",
        ScVal::Symbol(_) => "Symbol",
        ScVal::U32(_) => "u32",
        ScVal::I32(_) => "i32",
        ScVal::U64(_) => "u64",
        ScVal::I64(_) => "i64",
        ScVal::U128(_) => "u128",
        ScVal::Void => "void",
        ScVal::Vec(_) => "Vec",
        ScVal::Map(_) => "Map",
        ScVal::String(_) => "String",
        ScVal::Bytes(_) => "Bytes",
        _ => "other ScVal",
    }
}

/// The data payload of an event, unwrapped from its XDR body.
pub(crate) fn event_data(event: &soroban_sdk::xdr::ContractEvent) -> soroban_sdk::xdr::ScVal {
    let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
    body.data.clone()
}

/// A readable rendering of a payload, for the message a failed assertion prints.
///
/// Scalars and the inside of maps and vectors are spelled out because those are
/// the values a test author compares against; everything rarer falls back to the
/// XDR debug form so a message never hides the payload it rejected.
pub(crate) fn data_label(env: &Env, value: &soroban_sdk::xdr::ScVal) -> String {
    use soroban_sdk::xdr::ScVal;

    match value {
        ScVal::Bool(value) => value.to_string(),
        ScVal::Void => "void".to_string(),
        ScVal::U32(value) => value.to_string(),
        ScVal::I32(value) => value.to_string(),
        ScVal::U64(value) => value.to_string(),
        ScVal::I64(value) => value.to_string(),
        ScVal::U128(parts) => {
            let value = (parts.hi as u128) << 64 | parts.lo as u128;
            value.to_string()
        }
        ScVal::I128(parts) => {
            let value = (parts.hi as i128) << 64 | (parts.lo as u128) as i128;
            value.to_string()
        }
        ScVal::Symbol(symbol) => {
            format!(
                "\"{}\"",
                String::from_utf8_lossy(AsRef::<[u8]>::as_ref(symbol))
            )
        }
        ScVal::String(text) => {
            format!(
                "\"{}\"",
                String::from_utf8_lossy(AsRef::<[u8]>::as_ref(text))
            )
        }
        ScVal::Bytes(bytes) => format!("bytes[{}]", bytes.len()),
        // An address is worth reading as the key a test recognises, and only the
        // env can turn the hash into one.
        ScVal::Address(_) => Address::try_from_val(env, value)
            .map(|address| format!("{address:?}"))
            .unwrap_or_else(|_| format!("{value:?}")),
        ScVal::Vec(Some(items)) => format!(
            "[{}]",
            items
                .iter()
                .map(|item| data_label(env, item))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ScVal::Map(Some(entries)) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|entry| format!(
                    "{}: {}",
                    data_label(env, &entry.key),
                    data_label(env, &entry.val)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => format!("{other:?}"),
    }
}

/// A readable form of an event for failure messages: its topic symbols, then
/// anything else about it a test author needs to spot the mistake.
fn describe(event: &soroban_sdk::xdr::ContractEvent) -> String {
    let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
    let topics = body
        .topics
        .iter()
        .map(topic_label)
        .collect::<Vec<_>>()
        .join(", ");
    format!("topics [{}], type {:?}", topics, event.type_)
}

fn topic_label(topic: &soroban_sdk::xdr::ScVal) -> String {
    match topic {
        soroban_sdk::xdr::ScVal::Symbol(symbol) => {
            String::from_utf8_lossy(AsRef::<[u8]>::as_ref(symbol)).into_owned()
        }
        soroban_sdk::xdr::ScVal::String(string) => {
            String::from_utf8_lossy(AsRef::<[u8]>::as_ref(string)).into_owned()
        }
        soroban_sdk::xdr::ScVal::U32(val) => val.to_string(),
        soroban_sdk::xdr::ScVal::I32(val) => val.to_string(),
        soroban_sdk::xdr::ScVal::U64(val) => val.to_string(),
        soroban_sdk::xdr::ScVal::I64(val) => val.to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{EventLog, EventMatcher};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{
        contract, contractevent, contractimpl, contracttype, map, symbol_short, vec, Address, Env,
        Symbol,
    };

    #[contractevent(topics = ["Transfer"])]
    #[derive(Clone, Debug, PartialEq)]
    pub struct TransferEvent {
        pub from: Address,
        pub to: Address,
        pub amount: i128,
    }

    #[contract]
    pub struct EmitterContract;

    #[contractimpl]
    impl EmitterContract {
        #[allow(deprecated)]
        pub fn emit(env: Env, topic: Symbol) {
            env.events().publish((topic,), ());
        }

        #[allow(deprecated)]
        pub fn emit_pair(env: Env, first: Symbol, second: Symbol) {
            env.events().publish((first, second), ());
        }

        pub fn emit_transfer(env: Env, from: Address, to: Address, amount: i128) {
            TransferEvent { from, to, amount }.publish(&env);
        }

        pub fn emit_multiple_transfers(env: Env, from: Address, to: Address) {
            TransferEvent {
                from: from.clone(),
                to: to.clone(),
                amount: 100,
            }
            .publish(&env);
            TransferEvent {
                from,
                to,
                amount: 1000,
            }
            .publish(&env);
        }

        pub fn silent(env: Env) {
            let _ = env;
        }
    }

    fn emitter(env: &Env) -> EmitterContractClient<'_> {
        let id = env.register(EmitterContract, ());
        EmitterContractClient::new(env, &id)
    }

    #[test]
    fn fresh_env_has_no_events() {
        let env = Env::default();
        EventMatcher::new(&env).assert_count(0);
    }

    #[test]
    fn counts_events_published_by_a_contract_call() {
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env).assert_count(1);
        EventMatcher::new(&env).assert_emitted();
    }

    #[test]
    fn from_contract_scopes_the_count_to_one_contract() {
        let env = Env::default();
        let first = emitter(&env);
        let second = emitter(&env);

        first.emit(&symbol_short!("ping"));

        EventMatcher::new(&env)
            .from_contract(&first.address)
            .assert_count(1);
        EventMatcher::new(&env)
            .from_contract(&second.address)
            .assert_count(0);
    }

    #[test]
    fn with_topic_selects_events_by_their_topic() {
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env).with_topic("ping").assert_count(1);
        EventMatcher::new(&env).with_topic("absent").assert_count(0);

        client.emit(&symbol_short!("pong"));

        EventMatcher::new(&env).with_topic("ping").assert_count(0);
        EventMatcher::new(&env).with_topic("pong").assert_count(1);
    }

    #[test]
    #[should_panic(expected = "Expected at least one event to be emitted")]
    fn assert_emitted_panics_without_events() {
        let env = Env::default();
        EventMatcher::new(&env).assert_emitted();
    }

    #[test]
    #[should_panic(expected = "Expected 2 events, found 0")]
    fn assert_count_reports_the_observed_total() {
        let env = Env::default();
        EventMatcher::new(&env).assert_count(2);
    }

    #[test]
    fn assert_not_emitted_passes_on_a_fresh_env() {
        let env = Env::default();
        EventMatcher::new(&env).assert_not_emitted();
    }

    #[test]
    fn assert_not_emitted_honours_the_topic_filter() {
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env)
            .with_topic("pong")
            .assert_not_emitted();
        EventMatcher::new(&env).with_topic("ping").assert_count(1);
    }

    #[test]
    fn assert_not_emitted_honours_the_contract_filter() {
        let env = Env::default();
        let first = emitter(&env);
        let second = emitter(&env);

        first.emit(&symbol_short!("ping"));

        EventMatcher::new(&env)
            .from_contract(&second.address)
            .assert_not_emitted();
    }

    #[test]
    #[should_panic(expected = "Expected no events to be emitted, found 1")]
    fn assert_not_emitted_fails_when_an_event_exists() {
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env).assert_not_emitted();
    }

    #[test]
    #[should_panic(expected = "topics [ping], type Contract")]
    fn assert_not_emitted_names_the_event_it_found() {
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env).assert_not_emitted();
    }

    #[test]
    fn assert_none_match_passes_when_no_event_qualifies() {
        use soroban_sdk::xdr::ContractEventType;
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env)
            .assert_none_match(|event| matches!(event.type_, ContractEventType::Diagnostic));
    }

    #[test]
    #[should_panic(expected = "Expected no event to match, found one: topics [ping]")]
    fn assert_none_match_names_the_offending_event() {
        use soroban_sdk::xdr::ContractEventType;
        let env = Env::default();
        let client = emitter(&env);

        client.emit(&symbol_short!("ping"));

        EventMatcher::new(&env)
            .assert_none_match(|event| matches!(event.type_, ContractEventType::Contract));
    }

    // Asserting on payloads needs a contract that publishes more than a topic, so
    // this one emits one event per data shape a test runs into in practice: a
    // scalar, a big number, a flag, a symbol, a vector, a map, and a struct.
    #[contracttype]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Moved {
        pub to: Address,
        pub amount: i128,
    }

    #[contractevent(topics = ["schema"])]
    #[derive(Clone)]
    pub struct SchemaEvent {
        pub amount: i128,
        pub count: u64,
        pub enabled: bool,
        pub name: Symbol,
        pub recipient: Address,
    }

    #[contract]
    pub struct DataEmitterContract;

    #[contractimpl]
    impl DataEmitterContract {
        #[allow(deprecated)]
        pub fn emit_scalars(env: Env, amount: i128) {
            env.events().publish((symbol_short!("count"),), 7u32);
            env.events().publish((symbol_short!("amount"),), amount);
            env.events().publish((symbol_short!("flag"),), true);
            env.events()
                .publish((symbol_short!("name"),), Symbol::new(&env, "widget"));
        }

        #[allow(deprecated)]
        pub fn emit_shapes(env: Env, to: Address) {
            env.events()
                .publish((symbol_short!("list"),), vec![&env, 1u32, 2u32]);
            env.events().publish(
                (symbol_short!("map"),),
                map![&env, (symbol_short!("fee"), 10u32)],
            );
            env.events()
                .publish((symbol_short!("moved"),), Moved { to, amount: 500 });
        }

        pub fn emit_schema(env: Env, recipient: Address) {
            SchemaEvent {
                amount: 500,
                count: 7,
                enabled: true,
                name: Symbol::new(&env, "widget"),
                recipient,
            }
            .publish(&env);
        }
    }

    fn data_emitter(env: &Env) -> DataEmitterContractClient<'_> {
        let id = env.register(DataEmitterContract, ());
        DataEmitterContractClient::new(env, &id)
    }

    #[test]
    fn assert_data_matches_deserializes_a_scalar() {
        let env = Env::default();
        let client = data_emitter(&env);

        client.emit_scalars(&9_000i128);

        EventMatcher::new(&env)
            .with_topic("count")
            .assert_data_matches(|data| data.deserialize::<u32>() == Some(7));
        EventMatcher::new(&env)
            .with_topic("amount")
            .assert_data_matches(|data| data.deserialize::<i128>() == Some(9_000));
    }

    #[test]
    fn assert_data_matches_deserializes_a_struct_published_by_a_contract() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);

        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("moved")
            .assert_data_matches(|data| {
                data.deserialize::<Moved>()
                    == Some(Moved {
                        to: to.clone(),
                        amount: 500,
                    })
            });
    }

    #[test]
    fn event_data_fields_enumerates_struct_fields_in_ledger_order() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);

        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("moved")
            .assert_data_matches(|data| {
                let names = data.fields();
                names == ["amount", "to"]
            });
    }

    #[test]
    fn event_data_fields_returns_empty_for_scalar_payload() {
        let env = Env::default();
        let client = data_emitter(&env);
        client.emit_scalars(&100);

        EventMatcher::new(&env)
            .with_topic("count")
            .assert_data_matches(|data| data.fields().is_empty());
    }

    #[test]
    fn event_data_fields_returns_empty_for_vector_payload() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);
        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("list")
            .assert_data_matches(|data| data.fields().is_empty());
    }

    #[test]
    fn assert_data_matches_reads_one_field_of_a_map_payload() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);

        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("moved")
            .assert_data_matches(|data| {
                data.field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(500)
            });
        EventMatcher::new(&env)
            .with_topic("moved")
            .assert_data_matches(|data| data.field("absent").is_none());
    }

    #[test]
    fn assert_data_matches_deserializes_a_vector() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);

        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("list")
            .assert_data_matches(|data| {
                data.deserialize::<soroban_sdk::Vec<u32>>()
                    .map(|items| items.len())
                    == Some(2)
            });
    }

    #[test]
    fn assert_data_matches_hands_the_raw_payload_over() {
        use soroban_sdk::xdr::ScVal;

        let env = Env::default();
        let client = data_emitter(&env);

        client.emit_scalars(&9_000i128);

        EventMatcher::new(&env)
            .with_topic("flag")
            .assert_data_matches(|data| matches!(data.raw(), ScVal::Bool(true)));
        EventMatcher::new(&env)
            .with_topic("name")
            .assert_data_matches(|data| matches!(data.raw(), ScVal::Symbol(_)));
    }

    #[test]
    fn assert_data_matches_applies_the_contract_filter() {
        let env = Env::default();
        let first = data_emitter(&env);

        first.emit_scalars(&1i128);

        EventMatcher::new(&env)
            .from_contract(&first.address)
            .assert_data_matches(|data| data.deserialize::<u32>() == Some(7));
    }

    #[test]
    #[should_panic(expected = "Expected an event whose data matches, but no event was emitted")]
    fn assert_data_matches_fails_when_nothing_is_in_scope() {
        let env = Env::default();
        EventMatcher::new(&env).assert_data_matches(|_| true);
    }

    #[test]
    #[should_panic(expected = "Expected an event whose data matches, but no event was emitted")]
    fn assert_data_matches_fails_when_the_contract_filter_leaves_nothing() {
        let env = Env::default();
        let first = data_emitter(&env);
        let second = data_emitter(&env);

        first.emit_scalars(&1i128);

        EventMatcher::new(&env)
            .from_contract(&second.address)
            .assert_data_matches(|_| true);
    }

    #[test]
    #[should_panic(expected = "checked 4 event(s) with data [7, 9000, true, \"widget\"]")]
    fn assert_data_matches_prints_every_payload_it_rejected() {
        let env = Env::default();
        let client = data_emitter(&env);

        client.emit_scalars(&9_000i128);

        EventMatcher::new(&env).assert_data_matches(|_| false);
    }

    #[test]
    #[should_panic(expected = "with data [{\"fee\": 10}]")]
    fn assert_data_matches_prints_the_fields_of_a_map_payload() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);

        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("map")
            .assert_data_matches(|_| false);
    }

    #[test]
    #[should_panic(expected = "with data [{\"amount\": 500, \"to\": Contract(")]
    fn assert_data_matches_prints_an_address_as_a_strkey() {
        let env = Env::default();
        let client = data_emitter(&env);
        let to = Address::generate(&env);

        client.emit_shapes(&to);

        EventMatcher::new(&env)
            .with_topic("moved")
            .assert_data_matches(|_| false);
    }

    // Aggregating needs more than one call, so these tests collect after each
    // invocation they mean to assert over and compare against the single-call
    // scope where the difference shows.
    #[test]
    fn event_log_aggregates_events_across_invocations() {
        let env = Env::default();
        let client = emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit(&symbol_short!("ping"));
        log.collect();
        client.emit(&symbol_short!("pong"));
        log.collect();

        log.matcher().assert_count(2);
        log.matcher().with_topic("ping").assert_count(1);
        log.matcher().with_topic("pong").assert_count(1);
        log.matcher().assert_emitted();

        // Without the log the same env still answers for one call only.
        EventMatcher::new(&env).assert_count(1);
        EventMatcher::new(&env).with_topic("ping").assert_count(0);
    }

    #[test]
    fn event_log_holds_only_the_calls_a_test_collected() {
        let env = Env::default();
        let client = emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit(&symbol_short!("ping"));
        // This call runs, and nobody collects it.
        client.emit(&symbol_short!("pong"));
        log.collect();

        log.matcher().assert_count(1);
        log.matcher().with_topic("ping").assert_not_emitted();
        log.matcher().with_topic("pong").assert_count(1);
    }

    #[test]
    fn event_log_records_nothing_for_a_call_that_emitted_nothing() {
        let env = Env::default();
        let client = emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit(&symbol_short!("ping"));
        log.collect();
        client.silent();
        log.collect();

        log.matcher().assert_count(1);
        log.matcher().with_topic("pong").assert_not_emitted();
    }

    #[test]
    fn event_log_starts_empty_and_exposes_what_it_collected() {
        let env = Env::default();
        let mut log = EventLog::new(&env);

        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
        assert!(log.topics().is_empty());
        log.matcher().assert_not_emitted();

        let client = emitter(&env);
        client.emit(&symbol_short!("ping"));
        log.collect();

        assert_eq!(log.len(), 1);
        assert!(!log.is_empty());
        assert_eq!(log.events().len(), 1);
    }

    #[test]
    fn event_log_scopes_the_contract_filter_over_the_whole_sequence() {
        let env = Env::default();
        let first = emitter(&env);
        let second = emitter(&env);
        let mut log = EventLog::new(&env);

        first.emit(&symbol_short!("ping"));
        log.collect();
        second.emit(&symbol_short!("ping"));
        log.collect();

        log.matcher().from_contract(&first.address).assert_count(1);
        log.matcher().assert_count(2);

        // The latest call came from `second`, so a single-scope matcher filtered
        // to `first` sees nothing at all.
        EventMatcher::new(&env)
            .from_contract(&first.address)
            .assert_count(0);
    }

    #[test]
    fn from_contract_with_an_account_address_matches_nothing() {
        let env = Env::default();
        let client = emitter(&env);
        let account = Address::generate(&env);

        client.emit(&symbol_short!("ping"));

        let mut log = EventLog::new(&env);
        log.collect();

        EventMatcher::new(&env)
            .from_contract(&account)
            .assert_not_emitted();
        log.matcher().from_contract(&account).assert_not_emitted();
    }

    #[test]
    fn event_log_reads_the_topic_sequence_back_in_call_order() {
        let env = Env::default();
        let client = emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit(&symbol_short!("ping"));
        log.collect();
        client.emit_pair(&symbol_short!("claim"), &symbol_short!("grant"));
        log.collect();

        let sequence = log
            .topics()
            .iter()
            .map(|topics| topics.join("."))
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(sequence, "ping|claim.grant");
    }

    #[test]
    fn event_log_asserts_over_every_collected_payload() {
        let env = Env::default();
        let client = data_emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit_scalars(&1i128);
        log.collect();
        client.emit_scalars(&2i128);
        log.collect();

        log.matcher().with_topic("amount").assert_count(2);
        log.matcher()
            .with_topic("amount")
            .assert_data_matches(|data| data.deserialize::<i128>() == Some(1));
        log.matcher()
            .with_topic("amount")
            .assert_data_matches(|data| data.deserialize::<i128>() == Some(2));
    }

    #[test]
    #[should_panic(expected = "checked 2 event(s) with data [1, 2]")]
    fn event_log_prints_the_payloads_of_every_collected_call() {
        let env = Env::default();
        let client = data_emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit_scalars(&1i128);
        log.collect();
        client.emit_scalars(&2i128);
        log.collect();

        log.matcher()
            .with_topic("amount")
            .assert_data_matches(|_| false);
    }

    #[test]
    #[should_panic(expected = "Expected 3 events, found 2")]
    fn event_log_reports_the_aggregate_when_a_count_assertion_fails() {
        let env = Env::default();
        let client = emitter(&env);
        let mut log = EventLog::new(&env);

        client.emit(&symbol_short!("ping"));
        log.collect();
        client.emit(&symbol_short!("pong"));
        log.collect();

        log.matcher().assert_count(3);
    }

    // `assert_event_count_range!` / `assert_event_topic_count!` and the matcher
    // methods behind them.

    #[contract]
    pub struct BatchContract;

    #[contractimpl]
    impl BatchContract {
        /// Publishes `n` transfers, then one `fee` event and one `audit` event,
        /// the auxiliary events that make exact counts brittle.
        #[allow(deprecated)]
        pub fn batch(env: Env, to: Address, n: u32) {
            for _ in 0..n {
                env.events()
                    .publish((symbol_short!("transfer"), to.clone()), 1i128);
            }
            env.events().publish((symbol_short!("fee"),), 1i128);
            env.events().publish((symbol_short!("audit"),), ());
        }
    }

    fn batch(env: &Env) -> BatchContractClient<'_> {
        let id = env.register(BatchContract, ());
        BatchContractClient::new(env, &id)
    }

    #[test]
    fn count_range_accepts_every_range_shape_that_contains_the_count() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2); // 2 transfers + fee + audit = 4

        crate::assert_event_count_range!(env, 4);
        crate::assert_event_count_range!(env, 3..=5);
        crate::assert_event_count_range!(env, 4..=4);
        crate::assert_event_count_range!(env, 1..5);
        crate::assert_event_count_range!(env, 4..);
        crate::assert_event_count_range!(env, ..5);
        crate::assert_event_count_range!(env, ..=4);
        crate::assert_event_count_range!(env, ..);
        crate::assert_event_count_range!(&env, 3usize..=5usize);
        crate::assert_event_count_range!(env, 3u32..=5u32);
    }

    #[test]
    #[should_panic(expected = "Event count out of range: expected 5..=8 event(s), observed 4")]
    fn count_range_fails_below_the_lower_bound() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_count_range!(env, 5..=8);
    }

    #[test]
    #[should_panic(expected = "expected ..=3 event(s), observed 4")]
    fn count_range_fails_above_the_upper_bound() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_count_range!(env, ..=3);
    }

    #[test]
    #[should_panic(expected = "expected 1..4 event(s), observed 4")]
    fn count_range_treats_an_exclusive_end_as_exclusive() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_count_range!(env, 1..4);
    }

    #[test]
    #[should_panic(expected = "expected exactly 3 event(s), observed 4")]
    fn count_range_accepts_an_exact_count() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_count_range!(env, 3);
    }

    #[test]
    #[should_panic(expected = "| transfer, <address> |     2 |")]
    fn count_range_failure_prints_a_table_of_observed_topics() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_count_range!(env, 10..);
    }

    #[test]
    fn count_range_failure_table_lists_each_signature_in_first_seen_order() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &3);

        let message = panic_message(|| crate::assert_event_count_range!(env, ..2));
        let expected = "\
Scope: most recent invocation
| Observed topics     | Count |
|---------------------|-------|
| transfer, <address> |     3 |
| fee                 |     1 |
| audit               |     1 |
| total               |     5 |
";
        assert!(message.ends_with(expected), "unexpected table:\n{message}");
    }

    #[test]
    #[should_panic(expected = "| (none)          |     0 |")]
    fn count_range_failure_on_a_silent_call_says_nothing_was_observed() {
        let env = Env::default();
        crate::assert_event_count_range!(env, 1..=2);
    }

    #[test]
    fn count_range_with_from_counts_one_contract_only() {
        let env = Env::default();
        let first = batch(&env);
        let other = batch(&env);
        let to = Address::generate(&env);

        first.batch(&to, &1);

        crate::assert_event_count_range!(env, from = first.address, 3);
        crate::assert_event_count_range!(env, from = other.address, 0);
        crate::assert_event_count_range!(env, from = &first.address, 2..=3);
    }

    #[test]
    fn count_range_honours_the_topic_filter_of_a_matcher() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &3);

        EventMatcher::new(&env)
            .with_topic("transfer")
            .assert_count_within(2..=3);
    }

    #[test]
    #[should_panic(expected = "expected 4.. event(s) with topic \"transfer\", observed 3")]
    fn count_range_names_the_topic_filter_when_it_fails() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &3);

        EventMatcher::new(&env)
            .with_topic("transfer")
            .assert_count_within(4..);
    }

    #[test]
    fn count_range_reads_an_event_log_across_invocations() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);
        let mut log = EventLog::new(&env);

        client.batch(&to, &1);
        log.collect();
        client.batch(&to, &2);
        log.collect();

        // 3 + 4 events collected; the latest call alone has 4.
        crate::assert_event_count_range!(log, 7);
        crate::assert_event_count_range!(&log, 5..=10);
        crate::assert_event_count_range!(env, 4);
        crate::assert_event_topic_count!(log, "transfer", 3);
        crate::assert_event_topic_count!(log, "fee", 2);
    }

    #[test]
    #[should_panic(expected = "Scope: event log (7 collected)")]
    fn count_range_failure_over_a_log_names_the_log_as_its_scope() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);
        let mut log = EventLog::new(&env);

        client.batch(&to, &1);
        log.collect();
        client.batch(&to, &2);
        log.collect();

        crate::assert_event_count_range!(log, ..7);
    }

    #[test]
    fn topic_count_matches_exact_frequencies() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_topic_count!(env, "transfer", 2);
        crate::assert_event_topic_count!(env, "fee", 1);
        crate::assert_event_topic_count!(env, "audit", 1);
        crate::assert_event_topic_count!(env, "refund", 0);
    }

    #[test]
    fn topic_count_accepts_ranges() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_topic_count!(env, "transfer", 1..=3);
        crate::assert_event_topic_count!(env, "transfer", 2..);
        crate::assert_event_topic_count!(env, "refund", ..1);
    }

    #[test]
    #[should_panic(
        expected = "Event topic count mismatch: expected topic \"transfer\" exactly 3 time(s), observed 2"
    )]
    fn topic_count_fails_on_a_frequency_mismatch() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_topic_count!(env, "transfer", 3);
    }

    #[test]
    #[should_panic(expected = "observed 0 (topic \"Transfer\" was not observed)")]
    fn topic_count_reports_a_missing_topic() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        // Symbols are case sensitive: the contract published `transfer`.
        crate::assert_event_topic_count!(env, "Transfer", 2);
    }

    #[test]
    fn topic_count_failure_includes_the_observed_topics_table() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &1);

        let message = panic_message(|| crate::assert_event_topic_count!(env, "mint", 1));
        assert!(message.contains("expected topic \"mint\" exactly 1 time(s), observed 0"));
        assert!(message.contains("| transfer, <address> |     1 |"));
        assert!(message.contains("| fee                 |     1 |"));
        assert!(message.contains("| total               |     3 |"));
    }

    #[test]
    #[should_panic(expected = "expected topic \"fee\" 2..=3 time(s), observed 1")]
    fn topic_count_fails_when_a_range_is_breached() {
        let env = Env::default();
        let client = batch(&env);
        let to = Address::generate(&env);

        client.batch(&to, &2);

        crate::assert_event_topic_count!(env, "fee", 2..=3);
    }

    #[test]
    fn topic_count_with_from_counts_one_contract_only() {
        let env = Env::default();
        let first = batch(&env);
        let other = batch(&env);
        let to = Address::generate(&env);

        first.batch(&to, &2);

        crate::assert_event_topic_count!(env, from = first.address, "transfer", 2);
        crate::assert_event_topic_count!(env, from = other.address, "transfer", 0);
    }

    #[test]
    fn topic_count_matches_a_symbol_in_any_topic_position() {
        let env = Env::default();
        let client = emitter(&env);

        client.emit_pair(&symbol_short!("claim"), &symbol_short!("grant"));

        crate::assert_event_topic_count!(env, "claim", 1);
        crate::assert_event_topic_count!(env, "grant", 1);
    }

    #[test]
    fn a_negative_bound_matches_no_count() {
        use super::EventCountBound;

        assert!(!(-1i32).contains_count(0));
        assert!(!(..-1i32).contains_count(0));
        assert!((-5i32..=0).contains_count(0));
        assert_eq!((..).describe(), "any number (..)");
        assert_eq!(2usize.describe(), "exactly 2");
    }

    /// Runs `f`, which must panic, and returns its panic message.
    fn panic_message(f: impl FnOnce()) -> String {
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .expect_err("the assertion was expected to fail");
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    #[test]
    fn assert_event_data_schema_accepts_expected_struct_field_types() {
        let env = Env::default();
        let client = data_emitter(&env);
        let recipient = Address::generate(&env);
        client.emit_schema(&recipient);

        crate::assert_event_data_schema!(&env, "schema", {
            amount: i128,
            count: u64,
            enabled: bool,
            name: Symbol,
            recipient: Address,
        });
    }

    #[test]
    #[should_panic(expected = "missing field `absent` (expected Symbol)")]
    fn assert_event_data_schema_reports_missing_fields() {
        let env = Env::default();
        let client = data_emitter(&env);
        let recipient = Address::generate(&env);
        client.emit_schema(&recipient);

        crate::assert_event_data_schema!(&env, "schema", {
            absent: Symbol,
        });
    }

    #[test]
    #[should_panic(expected = "field `amount` expected u64, found i128")]
    fn assert_event_data_schema_reports_wrong_field_types() {
        let env = Env::default();
        let client = data_emitter(&env);
        let recipient = Address::generate(&env);
        client.emit_schema(&recipient);

        crate::assert_event_data_schema!(&env, "schema", {
            amount: u64,
            count: u64,
            enabled: bool,
            name: Symbol,
            recipient: Address,
        });
    }

    #[test]
    #[should_panic(expected = "unexpected field `to`")]
    fn assert_event_data_schema_rejects_extra_fields() {
        let env = Env::default();
        let client = data_emitter(&env);
        let recipient = Address::generate(&env);
        client.emit_shapes(&recipient);

        crate::assert_event_data_schema!(&env, "moved", {
            amount: i128,
        });
    }

    #[test]
    fn assert_event_emitted_matches_contract_topic_and_fields() {
        let env = Env::default();
        let client = emitter(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        client.emit_transfer(&from, &to, &1000i128);

        crate::assert_event_emitted!(env, client.address, "Transfer", {
            from: from,
            to: to,
            amount: 1000i128,
        });
    }

    #[test]
    fn assert_event_emitted_matches_contract_and_topic_only() {
        let env = Env::default();
        let client = emitter(&env);
        client.emit(&symbol_short!("ping"));

        crate::assert_event_emitted!(env, client.address, "ping");
    }

    #[test]
    fn assert_event_emitted_matches_multiple_topics() {
        let env = Env::default();
        let client = emitter(&env);
        client.emit_pair(&symbol_short!("claim"), &symbol_short!("grant"));

        crate::assert_event_emitted!(env, client.address, ["claim", "grant"]);
    }

    #[test]
    fn assert_event_emitted_matches_multiple_topics_and_fields() {
        let env = Env::default();
        let client = emitter(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        client.emit_transfer(&from, &to, &1000i128);

        crate::assert_event_emitted!(env, client.address, ["Transfer"], {
            from: from,
            to: to,
            amount: 1000i128,
        });
    }

    #[test]
    fn assert_event_emitted_succeeds_when_multiple_events_match() {
        let env = Env::default();
        let client = emitter(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        client.emit_multiple_transfers(&from, &to);

        // At least one event matches each assertion
        crate::assert_event_emitted!(env, client.address, "Transfer", {
            amount: 1000i128,
        });
        crate::assert_event_emitted!(env, client.address, "Transfer", {
            amount: 100i128,
        });
    }

    #[test]
    #[should_panic(expected = "assert_event_emitted! failed: no matching event found.")]
    fn assert_event_emitted_fails_on_data_mismatch() {
        let env = Env::default();
        let client = emitter(&env);
        let from = Address::generate(&env);
        let to = Address::generate(&env);

        client.emit_transfer(&from, &to, &500i128);

        crate::assert_event_emitted!(env, client.address, "Transfer", {
            amount: 1000i128,
        });
    }

    #[test]
    #[should_panic(expected = "assert_event_emitted! failed: no matching event found.")]
    fn assert_event_emitted_fails_on_topic_mismatch() {
        let env = Env::default();
        let client = emitter(&env);
        client.emit(&symbol_short!("ping"));

        crate::assert_event_emitted!(env, client.address, "pong");
    }

    #[test]
    #[should_panic(expected = "assert_event_emitted! failed: no matching event found.")]
    fn assert_event_emitted_fails_on_contract_mismatch() {
        let env = Env::default();
        let first = emitter(&env);
        let second = emitter(&env);
        first.emit(&symbol_short!("ping"));

        crate::assert_event_emitted!(env, second.address, "ping");
    }

    #[test]
    #[should_panic(expected = "(no events emitted in the most recent invocation)")]
    fn assert_event_emitted_fails_on_empty_events() {
        let env = Env::default();
        let client = emitter(&env);
        client.silent();

        crate::assert_event_emitted!(env, client.address, "Transfer");
    }

    #[test]
    fn with_topics_matches_events_with_all_specified_topics() {
        let env = Env::default();
        let client = emitter(&env);
        client.emit_pair(&symbol_short!("claim"), &symbol_short!("grant"));

        EventMatcher::new(&env)
            .with_topics(["claim", "grant"])
            .assert_count(1);
        EventMatcher::new(&env)
            .with_topics(["claim", "missing"])
            .assert_count(0);
    }
}
