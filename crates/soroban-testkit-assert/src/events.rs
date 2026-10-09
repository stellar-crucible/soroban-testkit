use soroban_sdk::testutils::Events as _;
use soroban_sdk::{Address, Env, TryFromVal};

/// Ergonomic assertions over the events a contract published.
///
/// The SDK exposes only the events of the most recent contract invocation
/// through `env.events().all()`, so a matcher always reads the latest call.
/// Assert between invocations rather than accumulating counts across a test.
pub struct EventMatcher<'a> {
    env: &'a Env,
    contract: Option<Address>,
    topic: Option<String>,
}

impl<'a> EventMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            contract: None,
            topic: None,
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

    /// Applies the configured filters to the events of the latest invocation.
    fn matched(&self) -> Vec<soroban_sdk::xdr::ContractEvent> {
        let all = self.env.events().all();
        let scoped = match &self.contract {
            Some(contract) => all.filter_by_contract(contract),
            None => all,
        };

        let mut events = scoped.events().to_vec();
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

    /// Fail when the latest invocation published anything in scope.
    ///
    /// Honours [`Self::from_contract`] and [`Self::with_topic`], so
    /// `with_topic("Transfer").assert_not_emitted()` reads as "no Transfer event
    /// came out of that call".
    ///
    /// Scope is the most recent contract invocation only — see the note on
    /// [`EventMatcher`]. Proving an event never appeared across a whole test
    /// needs aggregation, tracked as
    /// [issue #19](https://github.com/stellar-crucible/soroban-testkit/issues/19).
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
    use super::EventMatcher;
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
}
