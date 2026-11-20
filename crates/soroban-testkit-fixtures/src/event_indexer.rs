//! An indexed event store for multi-ledger test scenarios.
//!
//! SDK v28 exposes events for the most recent invocation. Ingest each
//! invocation explicitly and supply the ledger sequence from the scenario:
//!
//! ```
//! use soroban_sdk::{
//!     contract, contractimpl, symbol_short,
//!     testutils::{Events as _, Ledger as _},
//!     Env,
//! };
//! use soroban_testkit_fixtures::EventIndexerEmulator;
//!
//! mod publisher {
//!     use soroban_sdk::{contract, contractimpl, symbol_short, Env};
//!
//!     #[contract]
//!     pub struct Publisher;
//!
//!     #[contractimpl]
//!     impl Publisher {
//!         #[allow(deprecated)]
//!         pub fn publish(env: Env) {
//!             env.events().publish((symbol_short!("transfer"),), 1u32);
//!         }
//!     }
//! }
//!
//! let env = Env::default();
//! let id = env.register(publisher::Publisher, ());
//! let client = publisher::PublisherClient::new(&env, &id);
//! let mut indexer = EventIndexerEmulator::new();
//! let ledger = 42;
//! env.ledger().set_sequence_number(ledger);
//! client.publish();
//! indexer
//!     .ingest_ledger(ledger, env.events().all().events())
//!     .unwrap();
//!
//! let page = indexer.query_by_topic("trans*");
//! assert_eq!(page.total, 1);
//! assert_eq!(page.events[0].identity.ledger_sequence, ledger);
//! ```

use soroban_sdk::{
    xdr::{ContractEvent, ContractEventBody, ContractId, ScVal},
    Address,
};
use std::collections::{BTreeMap, BTreeSet};

/// A fixture-level identity for an ingested event.
///
/// Soroban `ContractEvent` does not expose a unique event id. The indexer
/// therefore identifies an occurrence by its ledger sequence and its
/// monotonically assigned ingestion ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventIdentity {
    pub ledger_sequence: u32,
    pub ingestion_ordinal: u64,
}

/// An event together with the ledger sequence supplied when it was ingested.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedEvent {
    pub identity: EventIdentity,
    pub event: ContractEvent,
}

/// A page of matching events in deterministic ledger and ingestion order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventPage {
    pub events: Vec<IndexedEvent>,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
}

impl EventPage {
    /// Whether another page of matching events remains after this one.
    pub fn has_more(&self) -> bool {
        self.offset.saturating_add(self.events.len()) < self.total
    }

    /// The offset to use for the next page, if one remains.
    pub fn next_offset(&self) -> Option<usize> {
        self.has_more()
            .then(|| self.offset.saturating_add(self.events.len()))
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// The filters applied together by [`EventIndexerEmulator::query`].
///
/// Multiple filters are combined with AND. A topic matches if any symbol topic
/// on an event satisfies its pattern.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EventQuery {
    topic: Option<String>,
    contract_id: Option<ContractId>,
    ledger_range: Option<(u32, u32)>,
    impossible_contract: bool,
}

impl EventQuery {
    pub fn new() -> Self {
        Self::default()
    }

    /// Match symbol topics against an exact string or a `*` glob pattern.
    ///
    /// `*` matches zero or more bytes; it may appear anywhere in the pattern.
    pub fn topic(mut self, pattern: impl Into<String>) -> Self {
        self.topic = Some(pattern.into());
        self
    }

    /// Restrict results to a Soroban contract id.
    pub fn contract_id(mut self, contract_id: &ContractId) -> Self {
        self.contract_id = Some(contract_id.clone());
        self
    }

    /// Restrict results to a contract address. Account addresses intentionally
    /// match no events because they are not event emitters.
    pub fn contract_address(mut self, address: &Address) -> Self {
        if let soroban_sdk::xdr::ScVal::Address(soroban_sdk::xdr::ScAddress::Contract(id)) =
            ScVal::from(address)
        {
            self.contract_id = Some(id);
        } else {
            self.impossible_contract = true;
        }
        self
    }

    /// Restrict results to an inclusive ledger sequence range.
    pub fn ledger_range(mut self, start_ledger: u32, end_ledger: u32) -> Self {
        self.ledger_range = Some((start_ledger, end_ledger));
        self
    }
}

/// Invalid query or index configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventIndexerError {
    InvalidLedgerRange { start: u32, end: u32 },
    ZeroPageLimit,
    IdentityOverflow,
}

impl std::fmt::Display for EventIndexerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLedgerRange { start, end } => {
                write!(f, "invalid inclusive ledger range {start}..={end}")
            }
            Self::ZeroPageLimit => write!(f, "page limit must be greater than zero"),
            Self::IdentityOverflow => write!(f, "event ingestion ordinal overflow"),
        }
    }
}

impl std::error::Error for EventIndexerError {}

/// An in-memory event index for an integration-test scenario.
///
/// Events are retained until this value is dropped. Ingestion is explicit:
/// pass the events of each completed invocation or ledger, because SDK v28
/// exposes the current invocation's events rather than a stream of ledger
/// closes. Each occurrence receives an identity based on the supplied ledger
/// and its ingestion order; identical XDR events ingested twice remain two
/// occurrences.
///
/// Topic and contract lookups use inverted indexes. Ledger-range lookups use
/// an ordered ledger index. Combined filters intersect indexed event identity
/// sets, so an event matching multiple of its own topics is still returned
/// once.
#[derive(Clone, Debug)]
pub struct EventIndexerEmulator {
    events: BTreeMap<EventIdentity, IndexedEvent>,
    topic_index: BTreeMap<Vec<u8>, BTreeSet<EventIdentity>>,
    contract_index: BTreeMap<ContractId, BTreeSet<EventIdentity>>,
    ledger_index: BTreeMap<u32, BTreeSet<EventIdentity>>,
    next_ordinal: u64,
    page_limit: usize,
}

impl Default for EventIndexerEmulator {
    fn default() -> Self {
        Self::new()
    }
}

impl EventIndexerEmulator {
    /// The default maximum number of events returned in one page.
    pub const DEFAULT_PAGE_LIMIT: usize = 100;

    pub fn new() -> Self {
        Self {
            events: BTreeMap::new(),
            topic_index: BTreeMap::new(),
            contract_index: BTreeMap::new(),
            ledger_index: BTreeMap::new(),
            next_ordinal: 0,
            page_limit: Self::DEFAULT_PAGE_LIMIT,
        }
    }

    /// Set the maximum number of results a query page can return.
    pub fn set_page_limit(&mut self, limit: usize) -> Result<(), EventIndexerError> {
        if limit == 0 {
            return Err(EventIndexerError::ZeroPageLimit);
        }
        self.page_limit = limit;
        Ok(())
    }

    pub fn page_limit(&self) -> usize {
        self.page_limit
    }

    /// Add events from one ledger or invocation to this scenario's index.
    ///
    /// The event data and emitter are preserved exactly as `ContractEvent`
    /// exposes them. Ledger sequence is fixture-level metadata supplied by the
    /// caller; it is not present in the SDK's `ContractEvent`.
    pub fn ingest_ledger(
        &mut self,
        ledger_sequence: u32,
        events: &[ContractEvent],
    ) -> Result<(), EventIndexerError> {
        let count = u64::try_from(events.len()).map_err(|_| EventIndexerError::IdentityOverflow)?;
        let next_ordinal = self
            .next_ordinal
            .checked_add(count)
            .ok_or(EventIndexerError::IdentityOverflow)?;

        for event in events {
            let identity = EventIdentity {
                ledger_sequence,
                ingestion_ordinal: self.next_ordinal,
            };
            let indexed = IndexedEvent {
                identity,
                event: event.clone(),
            };

            for topic in symbol_topics(event) {
                self.topic_index.entry(topic).or_default().insert(identity);
            }
            if let Some(contract_id) = &event.contract_id {
                self.contract_index
                    .entry(contract_id.clone())
                    .or_default()
                    .insert(identity);
            }
            self.ledger_index
                .entry(ledger_sequence)
                .or_default()
                .insert(identity);
            self.events.insert(identity, indexed);
            self.next_ordinal += 1;
        }
        debug_assert_eq!(self.next_ordinal, next_ordinal);
        Ok(())
    }

    /// Query the first page of symbol topics matching `pattern`.
    pub fn query_by_topic(&self, pattern: &str) -> EventPage {
        self.query(&EventQuery::new().topic(pattern))
            .expect("a topic-only query has no invalid range")
    }

    /// Query a selected page of symbol topics matching `pattern`.
    pub fn query_by_topic_page(
        &self,
        pattern: &str,
        offset: usize,
        limit: usize,
    ) -> Result<EventPage, EventIndexerError> {
        self.query_page(&EventQuery::new().topic(pattern), offset, limit)
    }

    /// Query the first page emitted by a contract address.
    ///
    /// Account addresses match no events. Use
    /// [`EventIndexerEmulator::query_by_contract_id`] when an XDR contract id
    /// is already available.
    pub fn query_by_contract(&self, contract: &Address) -> EventPage {
        self.query(&EventQuery::new().contract_address(contract))
            .expect("a contract-only query has no invalid range")
    }

    /// Query the first page for an XDR contract id.
    pub fn query_by_contract_id(&self, contract_id: &ContractId) -> EventPage {
        self.query(&EventQuery::new().contract_id(contract_id))
            .expect("a contract-only query has no invalid range")
    }

    /// Query a selected page emitted by a contract address.
    pub fn query_by_contract_page(
        &self,
        contract: &Address,
        offset: usize,
        limit: usize,
    ) -> Result<EventPage, EventIndexerError> {
        self.query_page(&EventQuery::new().contract_address(contract), offset, limit)
    }

    /// Query a selected page for an XDR contract id.
    pub fn query_by_contract_id_page(
        &self,
        contract_id: &ContractId,
        offset: usize,
        limit: usize,
    ) -> Result<EventPage, EventIndexerError> {
        self.query_page(&EventQuery::new().contract_id(contract_id), offset, limit)
    }

    /// Query the first page in the inclusive ledger range `start_ledger..=end_ledger`.
    pub fn query_range(
        &self,
        start_ledger: u32,
        end_ledger: u32,
    ) -> Result<EventPage, EventIndexerError> {
        self.query(&EventQuery::new().ledger_range(start_ledger, end_ledger))
    }

    /// Query a selected page in the inclusive ledger range.
    pub fn query_range_page(
        &self,
        start_ledger: u32,
        end_ledger: u32,
        offset: usize,
        limit: usize,
    ) -> Result<EventPage, EventIndexerError> {
        self.query_page(
            &EventQuery::new().ledger_range(start_ledger, end_ledger),
            offset,
            limit,
        )
    }

    /// Run several indexed filters together, returning one page.
    pub fn query(&self, query: &EventQuery) -> Result<EventPage, EventIndexerError> {
        self.query_page(query, 0, self.page_limit)
    }

    /// Run several indexed filters together with the requested pagination.
    ///
    /// Requested limits larger than the configured page limit are capped.
    /// A zero requested limit is invalid. Results are ordered by ledger
    /// sequence, then ingestion order within each ledger.
    pub fn query_page(
        &self,
        query: &EventQuery,
        offset: usize,
        limit: usize,
    ) -> Result<EventPage, EventIndexerError> {
        if limit == 0 {
            return Err(EventIndexerError::ZeroPageLimit);
        }
        if let Some((start, end)) = query.ledger_range {
            if start > end {
                return Err(EventIndexerError::InvalidLedgerRange { start, end });
            }
        }

        let mut candidates: Option<BTreeSet<EventIdentity>> = None;
        if query.impossible_contract {
            candidates = Some(BTreeSet::new());
        }
        if let Some(pattern) = &query.topic {
            let matches = self
                .topic_index
                .iter()
                .filter(|(topic, _)| topic_matches(pattern.as_bytes(), topic))
                .flat_map(|(_, identities)| identities.iter().copied())
                .collect();
            intersect(&mut candidates, matches);
        }
        if let Some(contract_id) = &query.contract_id {
            let matches = self
                .contract_index
                .get(contract_id)
                .cloned()
                .unwrap_or_default();
            intersect(&mut candidates, matches);
        }
        if let Some((start, end)) = query.ledger_range {
            let matches = self
                .ledger_index
                .range(start..=end)
                .flat_map(|(_, identities)| identities.iter().copied())
                .collect();
            intersect(&mut candidates, matches);
        }

        let matches = candidates.unwrap_or_else(|| self.events.keys().copied().collect());
        let total = matches.len();
        let effective_limit = limit.min(self.page_limit);
        let events = matches
            .into_iter()
            .skip(offset)
            .take(effective_limit)
            .filter_map(|identity| self.events.get(&identity).cloned())
            .collect();

        Ok(EventPage {
            events,
            offset,
            limit: effective_limit,
            total,
        })
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

fn intersect(candidates: &mut Option<BTreeSet<EventIdentity>>, matches: BTreeSet<EventIdentity>) {
    if let Some(candidates) = candidates {
        *candidates = candidates.intersection(&matches).copied().collect();
    } else {
        *candidates = Some(matches);
    }
}

fn symbol_topics(event: &ContractEvent) -> Vec<Vec<u8>> {
    let ContractEventBody::V0(body) = &event.body;
    body.topics
        .iter()
        .filter_map(|topic| match topic {
            ScVal::Symbol(symbol) => Some(AsRef::<[u8]>::as_ref(symbol).to_vec()),
            _ => None,
        })
        .collect()
}

fn topic_matches(pattern: &[u8], topic: &[u8]) -> bool {
    let (mut pattern_index, mut topic_index) = (0, 0);
    let mut last_star = None;
    let mut retry_topic_index = 0;

    while topic_index < topic.len() {
        if pattern.get(pattern_index) == Some(&topic[topic_index]) {
            pattern_index += 1;
            topic_index += 1;
        } else if pattern.get(pattern_index) == Some(&b'*') {
            last_star = Some(pattern_index);
            pattern_index += 1;
            retry_topic_index = topic_index;
        } else if let Some(star_index) = last_star {
            pattern_index = star_index + 1;
            retry_topic_index += 1;
            topic_index = retry_topic_index;
        } else {
            return false;
        }
    }

    while pattern.get(pattern_index) == Some(&b'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::{EventIndexerEmulator, EventIndexerError, EventQuery};
    use soroban_sdk::{
        contract, contractimpl, symbol_short,
        testutils::{Address as _, Events as _, Ledger as _},
        xdr::{ContractId, ScAddress},
        Address, Env, Symbol,
    };

    #[contract]
    struct Emitter;

    #[contractimpl]
    impl Emitter {
        #[allow(deprecated)]
        pub fn emit(env: Env, topic: Symbol) {
            env.events().publish((topic,), 1u32);
        }

        #[allow(deprecated)]
        pub fn emit_two(env: Env, first: Symbol, second: Symbol) {
            env.events().publish((first, second), 2u32);
        }
    }

    fn xdr_contract_id(address: &Address) -> ContractId {
        let soroban_sdk::xdr::ScVal::Address(ScAddress::Contract(id)) =
            soroban_sdk::xdr::ScVal::from(address)
        else {
            panic!("expected a contract address");
        };
        id
    }

    fn emit(env: &Env, contract_id: &Address, topic: &str) -> Vec<soroban_sdk::xdr::ContractEvent> {
        let client = EmitterClient::new(env, contract_id);
        client.emit(&Symbol::new(env, topic));
        env.events().all().events().to_vec()
    }

    #[test]
    fn indexes_events_from_multiple_ledgers_and_contracts() {
        let env = Env::default();
        let first = env.register(Emitter, ());
        let second = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();

        env.ledger().set_sequence_number(10);
        indexer
            .ingest_ledger(10, &emit(&env, &first, "opened"))
            .unwrap();
        env.ledger().set_sequence_number(11);
        indexer
            .ingest_ledger(11, &emit(&env, &second, "closed"))
            .unwrap();

        assert_eq!(indexer.len(), 2);
        let page = indexer.query_range(10, 11).unwrap();
        assert_eq!(
            page.events
                .iter()
                .map(|record| record.identity.ledger_sequence)
                .collect::<Vec<_>>(),
            vec![10, 11]
        );
        assert_eq!(
            page.events[0].event.contract_id,
            Some(xdr_contract_id(&first))
        );
        assert_eq!(
            page.events[1].event.contract_id,
            Some(xdr_contract_id(&second))
        );
    }

    #[test]
    fn exact_topic_and_glob_queries_match_symbol_topics() {
        let env = Env::default();
        let contract_id = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();
        for topic in ["pay", "payment", "transfer"] {
            indexer
                .ingest_ledger(1, &emit(&env, &contract_id, topic))
                .unwrap();
        }

        let exact = indexer.query_by_topic("pay");
        assert_eq!(exact.total, 1);
        let prefix = indexer.query_by_topic("pay*");
        assert_eq!(prefix.total, 2);
        assert_eq!(indexer.query_by_topic("*").total, 3);
    }

    #[test]
    fn contract_lookup_uses_the_emitter_index() {
        let env = Env::default();
        let first = env.register(Emitter, ());
        let second = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();
        indexer
            .ingest_ledger(1, &emit(&env, &first, "first"))
            .unwrap();
        indexer
            .ingest_ledger(1, &emit(&env, &second, "second"))
            .unwrap();

        let page = indexer.query_by_contract(&first);
        assert_eq!(page.total, 1);
        assert_eq!(
            page.events[0].event.contract_id,
            Some(xdr_contract_id(&first))
        );
    }

    #[test]
    fn range_bounds_are_inclusive_and_reversed_ranges_are_rejected() {
        let env = Env::default();
        let contract_id = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();
        for ledger in 1..=3 {
            indexer
                .ingest_ledger(ledger, &emit(&env, &contract_id, "tick"))
                .unwrap();
        }

        assert_eq!(indexer.query_range(2, 3).unwrap().total, 2);
        assert_eq!(
            indexer.query_range(3, 2),
            Err(EventIndexerError::InvalidLedgerRange { start: 3, end: 2 })
        );
    }

    #[test]
    fn pages_are_ordered_and_requested_limits_are_capped() {
        let env = Env::default();
        let contract_id = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();
        indexer.set_page_limit(2).unwrap();
        for topic in ["one", "two", "three", "four"] {
            indexer
                .ingest_ledger(7, &emit(&env, &contract_id, topic))
                .unwrap();
        }

        let first = indexer.query_by_topic_page("*", 0, 99).unwrap();
        assert_eq!(first.limit, 2);
        assert!(first.has_more());
        let second = indexer
            .query_by_topic_page("*", first.next_offset().unwrap(), 99)
            .unwrap();
        assert_eq!(second.events.len(), 2);
        assert!(!second.has_more());
        assert!(
            first.events[0].identity.ingestion_ordinal < first.events[1].identity.ingestion_ordinal
        );
    }

    #[test]
    fn ledger_order_is_deterministic_even_if_ledgers_are_ingested_out_of_order() {
        let env = Env::default();
        let contract_id = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();
        indexer
            .ingest_ledger(9, &emit(&env, &contract_id, "later"))
            .unwrap();
        indexer
            .ingest_ledger(8, &emit(&env, &contract_id, "earlier"))
            .unwrap();
        indexer
            .ingest_ledger(9, &emit(&env, &contract_id, "also_later"))
            .unwrap();

        let events = indexer.query_by_topic("*").events;
        assert_eq!(
            events
                .iter()
                .map(|record| record.identity.ledger_sequence)
                .collect::<Vec<_>>(),
            vec![8, 9, 9]
        );
        assert!(events[1].identity.ingestion_ordinal < events[2].identity.ingestion_ordinal);
    }

    #[test]
    fn empty_results_and_zero_page_limits_are_explicit() {
        let mut indexer = EventIndexerEmulator::new();
        assert!(indexer.query_by_topic("absent").is_empty());
        assert_eq!(
            indexer.query_by_topic_page("*", 0, 0),
            Err(EventIndexerError::ZeroPageLimit)
        );
        assert_eq!(
            indexer.set_page_limit(0),
            Err(EventIndexerError::ZeroPageLimit)
        );
    }

    #[test]
    fn combined_filters_return_each_matching_event_once() {
        let env = Env::default();
        let first = env.register(Emitter, ());
        let second = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();

        let client = EmitterClient::new(&env, &first);
        client.emit_two(&symbol_short!("transfer"), &symbol_short!("transit"));
        indexer
            .ingest_ledger(4, env.events().all().events())
            .unwrap();
        indexer
            .ingest_ledger(5, &emit(&env, &first, "transfer"))
            .unwrap();
        indexer
            .ingest_ledger(4, &emit(&env, &second, "transfer"))
            .unwrap();

        let query = EventQuery::new()
            .topic("trans*")
            .contract_address(&first)
            .ledger_range(4, 4);
        let page = indexer.query(&query).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(
            page.events[0].event.contract_id,
            Some(xdr_contract_id(&first))
        );
    }

    #[test]
    fn account_addresses_do_not_match_contract_events() {
        let env = Env::default();
        let account = Address::generate(&env);
        let contract_id = env.register(Emitter, ());
        let mut indexer = EventIndexerEmulator::new();
        indexer
            .ingest_ledger(1, &emit(&env, &contract_id, "event"))
            .unwrap();

        assert!(indexer
            .query(&EventQuery::new().contract_address(&account))
            .unwrap()
            .is_empty());
    }
}
