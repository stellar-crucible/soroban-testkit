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
    topic: Option<String>,
    events: Option<&'a [soroban_sdk::xdr::ContractEvent]>,
}

impl<'a> EventMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            contract: None,
            topic: None,
            events: None,
        }
    }

    pub fn from_contract(mut self, contract: &Address) -> Self {
        self.contract = Some(contract.clone());
        self
    }

    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Applies the configured filters to the events in scope, whether that scope
    /// is the latest invocation or an [`EventLog`].
    fn matched(&self) -> Vec<soroban_sdk::xdr::ContractEvent> {
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

        if let Some(topic) = &self.topic {
            let bytes = topic.as_bytes();
            events.retain(|event| {
                let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
                body.topics.iter().any(|topic| match topic {
                    soroban_sdk::xdr::ScVal::Symbol(symbol) => {
                        AsRef::<[u8]>::as_ref(symbol) == bytes
                    }
                    _ => false,
                })
            });
        }
        events
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
            topic: None,
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
fn emitting_contract(address: &Address) -> Option<soroban_sdk::xdr::ContractId> {
    let soroban_sdk::xdr::ScVal::Address(emitter) = soroban_sdk::xdr::ScVal::from(address) else {
        unreachable!("an Address always converts to ScVal::Address");
    };
    match emitter {
        soroban_sdk::xdr::ScAddress::Contract(id) => Some(id),
        _ => None,
    }
}

/// The topic symbols of one event, as readable text.
fn event_topics(event: &soroban_sdk::xdr::ContractEvent) -> Vec<String> {
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
    fn new(env: &'a Env, raw: soroban_sdk::xdr::ScVal) -> Self {
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
}

/// The data payload of an event, unwrapped from its XDR body.
fn event_data(event: &soroban_sdk::xdr::ContractEvent) -> soroban_sdk::xdr::ScVal {
    let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
    body.data.clone()
}

/// A readable rendering of a payload, for the message a failed assertion prints.
///
/// Scalars and the inside of maps and vectors are spelled out because those are
/// the values a test author compares against; everything rarer falls back to the
/// XDR debug form so a message never hides the payload it rejected.
fn data_label(env: &Env, value: &soroban_sdk::xdr::ScVal) -> String {
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
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{EventLog, EventMatcher};
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{
        contract, contractimpl, contracttype, map, symbol_short, vec, Address, Env, Symbol,
    };

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
}
