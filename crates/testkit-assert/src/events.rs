use soroban_sdk::testutils::Events as _;
use soroban_sdk::{Address, Env};

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
}

#[cfg(test)]
mod tests {
    use super::EventMatcher;
    use soroban_sdk::{contract, contractimpl, symbol_short, Env, Symbol};

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
}
