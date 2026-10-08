use soroban_sdk::testutils::Events as _;
use soroban_sdk::Env;

pub struct EventMatcher<'a> {
    env: &'a Env,
    contract_id: Option<soroban_sdk::BytesN<32>>,
    topic_filter: Option<String>,
}

impl<'a> EventMatcher<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            contract_id: None,
            topic_filter: None,
        }
    }

    pub fn from_contract(mut self, contract_id: &soroban_sdk::BytesN<32>) -> Self {
        self.contract_id = Some(contract_id.clone());
        self
    }

    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic_filter = Some(topic.into());
        self
    }

    pub fn assert_emitted(&self) {
        let events = self.env.events().all();
        let event_list = events.events();
        assert!(
            !event_list.is_empty(),
            "Expected at least one event to be emitted"
        );
    }

    pub fn assert_count(&self, expected: usize) {
        let events = self.env.events().all();
        let event_list = events.events();
        assert_eq!(
            event_list.len(),
            expected,
            "Expected {} events, found {}",
            expected,
            event_list.len()
        );
    }
}
