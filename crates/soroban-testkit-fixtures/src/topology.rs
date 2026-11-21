//! Multi-contract dependency wiring and topology validation.
//!
//! A protocol suite (factory + router + pool + vault + token) is only healthy
//! when every contract holds the *live* address of each contract it depends
//! on. A contract registered with a stale or zero address still deploys and
//! still answers calls; the failure only shows up later, when the dependency is
//! invoked. [`TopologyValidator`] checks the wiring up front:
//!
//! * registers the contracts and the required dependency links of a protocol,
//! * scans each deployed contract's instance storage for address references,
//! * flags references that point at contracts that were never deployed
//!   (dangling) and declared links whose dependency address is missing from the
//!   holder's storage (uninitialized),
//! * renders the connected topology as Mermaid or a compact ASCII tree.
//!
//! ```rust
//! use soroban_sdk::{contract, contractimpl, symbol_short, testutils::Address as _, Address, Env};
//! use soroban_testkit_fixtures::topology::TopologyValidator;
//!
//! #[contract]
//! pub struct Pool;
//!
//! #[contractimpl]
//! impl Pool {
//!     pub fn initialize(env: Env, token: Address) {
//!         env.storage().instance().set(&symbol_short!("token"), &token);
//!     }
//! }
//!
//! # fn main() {
//! let env = Env::default();
//! env.mock_all_auths();
//! let token = env.register_stellar_asset_contract_v2(Address::generate(&env)).address();
//! let pool = env.register(Pool, ());
//! PoolClient::new(&env, &pool).initialize(&token);
//!
//! let mut topology = TopologyValidator::new(&env);
//! topology
//!     .register_contract("token", token.clone())
//!     .register_contract("pool", pool)
//!     .require_link("pool", "token");
//!
//! let report = topology.validate();
//! assert!(report.is_valid(), "{}", report.render_topology());
//! assert!(report.to_mermaid().contains("pool --> token"));
//! # }
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use soroban_sdk::{
    xdr::{LedgerEntryData, LedgerKey, ScAddress, ScVal},
    Address, Env, TryFromVal,
};

/// A contract participating in the protocol topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractNode {
    /// Stable name used in links and reports (`"pool"`, `"oracle"`, ...).
    pub name: String,
    /// The live on-chain address the name resolves to.
    pub address: Address,
}

/// A declared dependency: `from`'s instance storage must hold `to`'s address.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DependencyLink {
    pub from: String,
    pub to: String,
}

impl DependencyLink {
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

/// One contract address found in a contract's instance storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressReference {
    /// Name of the contract holding the reference.
    pub holder: String,
    /// Instance storage key the address was read from.
    pub key: String,
    /// The referenced address.
    pub referenced: Address,
}

/// A reference that points at an address no contract was deployed under, plus
/// references to contracts that were never registered in the topology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingReference {
    pub reference: AddressReference,
    /// Why the target is considered dangling.
    pub reason: String,
}

/// A declared link that validation could not wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninitializedLink {
    pub link: DependencyLink,
    /// Why the link is uninitialized.
    pub reason: String,
}

/// A registered contract that has no contract data on the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InactiveContract {
    pub name: String,
    pub address: Address,
}

/// Result of validating one protocol topology.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TopologyReport {
    /// Registered contracts, in registration order.
    pub nodes: Vec<ContractNode>,
    /// Declared links that were successfully wired.
    pub edges: Vec<DependencyLink>,
    /// Registered contracts with no deployed contract data.
    pub inactive_contracts: Vec<InactiveContract>,
    /// Address references pointing at undeployed contracts.
    pub dangling_references: Vec<DanglingReference>,
    /// Declared links whose address is absent from the holder's storage.
    pub uninitialized_links: Vec<UninitializedLink>,
    /// Links naming a contract that was never registered.
    pub unknown_links: Vec<DependencyLink>,
}

impl TopologyReport {
    /// True when every registered contract is deployed, every declared link is
    /// wired, and no address reference dangles.
    pub fn is_valid(&self) -> bool {
        self.inactive_contracts.is_empty()
            && self.dangling_references.is_empty()
            && self.uninitialized_links.is_empty()
            && self.unknown_links.is_empty()
    }

    /// Every problem found, one line each; empty when the topology is valid.
    pub fn violations(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.extend(self.inactive_contracts.iter().map(|inactive| {
            format!(
                "inactive contract `{}` at {:?}: no contract data on the ledger",
                inactive.name, inactive.address
            )
        }));
        out.extend(self.dangling_references.iter().map(|dangling| {
            format!(
                "dangling reference `{}` -> {:?} at instance key `{}`: {}",
                dangling.reference.holder,
                dangling.reference.referenced,
                dangling.reference.key,
                dangling.reason
            )
        }));
        out.extend(self.uninitialized_links.iter().map(|missing| {
            format!(
                "uninitialized link `{}` -> `{}`: {}",
                missing.link.from, missing.link.to, missing.reason
            )
        }));
        out.extend(self.unknown_links.iter().map(|link| {
            format!(
                "unknown link `{}` -> `{}`: one endpoint was never registered",
                link.from, link.to
            )
        }));
        out
    }

    /// Render the connected topology as a Mermaid flowchart.
    ///
    /// Wired dependency links are solid arrows; dangling references are dashed
    /// arrows to the missing address; inactive contracts are drawn with a
    /// warning marker.
    pub fn to_mermaid(&self) -> String {
        let mut out = String::from("flowchart TD\n");

        for node in &self.nodes {
            let inactive = self
                .inactive_contracts
                .iter()
                .any(|entry| entry.name == node.name);
            let suffix = if inactive { " (inactive)" } else { "" };
            out.push_str(&format!(
                "  {}[\"{}{}\"]\n",
                id(&node.name),
                node.name,
                suffix
            ));
        }

        for edge in &self.edges {
            out.push_str(&format!("  {} --> {}\n", id(&edge.from), id(&edge.to)));
        }

        for dangling in &self.dangling_references {
            out.push_str(&format!(
                "  {} -. \"dangling: {}\" .-> dangling_{}\n",
                id(&dangling.reference.holder),
                escape(&dangling.reference.key),
                id(&format!("{:?}", dangling.reference.referenced))
            ));
        }

        out
    }

    /// Compact human-readable summary: nodes, wired edges and every violation.
    pub fn render_topology(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Protocol topology: {} contracts, {} wired links\n",
            self.nodes.len(),
            self.edges.len()
        ));

        for node in &self.nodes {
            out.push_str(&format!("  {} -> {:?}\n", node.name, node.address));
            for edge in self.edges.iter().filter(|edge| edge.from == node.name) {
                out.push_str(&format!("    depends on {}\n", edge.to));
            }
        }

        let violations = self.violations();
        if violations.is_empty() {
            out.push_str("  topology valid\n");
        } else {
            out.push_str("Violations:\n");
            for violation in violations {
                out.push_str(&format!("  - {violation}\n"));
            }
        }

        out
    }

    /// Panic with a rendered topology and every violation when invalid.
    #[track_caller]
    pub fn assert_valid(&self) {
        assert!(
            self.is_valid(),
            "Protocol topology is not fully wired:\n{}",
            self.render_topology()
        );
    }
}

impl fmt::Display for TopologyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_topology())
    }
}

/// Declares a protocol's contract dependency graph and validates the live
/// address wiring inside each contract's instance storage.
pub struct TopologyValidator<'a> {
    env: &'a Env,
    nodes: Vec<ContractNode>,
    links: Vec<DependencyLink>,
    /// Extra addresses to accept as deployed even if they are not registered as
    /// nodes (e.g. a token the suite does not model as a protocol contract).
    external_deployed: BTreeSet<[u8; 32]>,
}

impl<'a> TopologyValidator<'a> {
    pub fn new(env: &'a Env) -> Self {
        Self {
            env,
            nodes: Vec::new(),
            links: Vec::new(),
            external_deployed: BTreeSet::new(),
        }
    }

    /// Register a contract under a stable name. Registering the same name twice
    /// replaces the address so an upgrade can be validated in place.
    pub fn register_contract(&mut self, name: impl Into<String>, address: Address) -> &mut Self {
        let name = name.into();
        if let Some(existing) = self.nodes.iter_mut().find(|node| node.name == name) {
            existing.address = address;
        } else {
            self.nodes.push(ContractNode { name, address });
        }
        self
    }

    /// Declare that `from` must hold `to`'s address in its instance storage.
    pub fn require_link(&mut self, from: impl Into<String>, to: impl Into<String>) -> &mut Self {
        let link = DependencyLink::new(from, to);
        if !self.links.contains(&link) {
            self.links.push(link);
        }
        self
    }

    /// Declare several dependencies of one contract, e.g.
    /// `require_links("pool", ["token", "oracle"])`.
    pub fn require_links<I, S>(&mut self, from: impl Into<String>, to: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let from = from.into();
        for target in to {
            self.require_link(from.clone(), target);
        }
        self
    }

    /// The contracts registered so far.
    pub fn contracts(&self) -> &[ContractNode] {
        &self.nodes
    }

    /// The declared links.
    pub fn links(&self) -> &[DependencyLink] {
        &self.links
    }

    /// Validate the declared topology against the current ledger state.
    pub fn validate(&self) -> TopologyReport {
        let by_name: BTreeMap<&str, &Address> = self
            .nodes
            .iter()
            .map(|node| (node.name.as_str(), &node.address))
            .collect();
        let deployed = deployed_contracts(self.env);

        let mut report = TopologyReport {
            nodes: self.nodes.clone(),
            ..TopologyReport::default()
        };

        for node in &self.nodes {
            let hash = contract_hash(self.env, &node.address);
            let is_deployed = hash
                .map(|hash| deployed.contains(&hash) || self.external_deployed.contains(&hash))
                .unwrap_or(false);
            if !is_deployed {
                report.inactive_contracts.push(InactiveContract {
                    name: node.name.clone(),
                    address: node.address.clone(),
                });
            }
        }

        // Only contracts that hold declared dependencies are scanned. A leaf
        // such as a token contract stores framework-internal addresses that are
        // outside the protocol topology and would otherwise be false positives.
        let holders: BTreeSet<&str> = self.links.iter().map(|link| link.from.as_str()).collect();

        let mut references: BTreeMap<String, Vec<AddressReference>> = BTreeMap::new();
        for node in self
            .nodes
            .iter()
            .filter(|node| holders.contains(node.name.as_str()))
        {
            let mut found = Vec::new();
            for (key, value) in instance_values(self.env, &node.address) {
                let mut addresses = Vec::new();
                collect_addresses(self.env, &value, &mut addresses);
                for referenced in addresses {
                    found.push(AddressReference {
                        holder: node.name.clone(),
                        key: key.clone(),
                        referenced,
                    });
                }
            }
            references.insert(node.name.clone(), found);
        }

        for (holder, found) in &references {
            for reference in found {
                let Some(hash) = contract_hash(self.env, &reference.referenced) else {
                    // Account addresses are not protocol contracts.
                    continue;
                };
                if !deployed.contains(&hash) && !self.external_deployed.contains(&hash) {
                    report.dangling_references.push(DanglingReference {
                        reference: reference.clone(),
                        reason: "the referenced contract was never deployed".to_string(),
                    });
                } else if !by_name
                    .values()
                    .any(|address| **address == reference.referenced)
                {
                    // Deployed, but the topology does not model it. That is a
                    // gap in the declared graph rather than a broken address.
                    report.dangling_references.push(DanglingReference {
                        reference: reference.clone(),
                        reason: format!(
                            "`{holder}` references a deployed contract missing from the topology"
                        ),
                    });
                }
            }
        }

        for link in &self.links {
            let (Some(from), Some(to)) = (
                by_name.get(link.from.as_str()),
                by_name.get(link.to.as_str()),
            ) else {
                report.unknown_links.push(link.clone());
                continue;
            };

            let from_inactive = report
                .inactive_contracts
                .iter()
                .any(|inactive| inactive.name == link.from);
            let to_inactive = report
                .inactive_contracts
                .iter()
                .any(|inactive| inactive.name == link.to);
            if from_inactive || to_inactive {
                report.uninitialized_links.push(UninitializedLink {
                    link: link.clone(),
                    reason: "one endpoint is not deployed".to_string(),
                });
                continue;
            }

            let wired = references
                .get(&link.from)
                .map(|found| found.iter().any(|reference| &reference.referenced == *to))
                .unwrap_or(false);
            if wired {
                report.edges.push(link.clone());
            } else {
                report.uninitialized_links.push(UninitializedLink {
                    link: link.clone(),
                    reason: format!(
                        "`{}` ({:?}) holds no instance-storage reference to `{}` ({:?})",
                        link.from, from, link.to, to
                    ),
                });
            }
        }

        report
    }

    /// Validate and panic with a full report when the topology is broken.
    #[track_caller]
    pub fn assert_valid(&self) -> TopologyReport {
        let report = self.validate();
        report.assert_valid();
        report
    }
}

/// The instance-storage key/value pairs of a deployed contract.
fn instance_values(env: &Env, contract: &Address) -> Vec<(String, ScVal)> {
    let Some(wanted) = contract_hash(env, contract) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for (key, (entry, _)) in env.to_ledger_snapshot().ledger_entries.iter() {
        let LedgerKey::ContractData(lookup) = &**key else {
            continue;
        };
        if contract_hash_of_sc(&lookup.contract) != Some(wanted) {
            continue;
        }
        let LedgerEntryData::ContractData(stored) = &entry.data else {
            continue;
        };
        let ScVal::LedgerKeyContractInstance = &stored.key else {
            continue;
        };
        let ScVal::ContractInstance(instance) = &stored.val else {
            continue;
        };
        let Some(storage) = &instance.storage else {
            continue;
        };
        for item in storage.0.iter() {
            found.push((render_key(&item.key), item.val.clone()));
        }
    }
    found
}

/// Every contract id that owns at least one contract-data ledger entry.
fn deployed_contracts(env: &Env) -> BTreeSet<[u8; 32]> {
    let mut deployed = BTreeSet::new();
    for (key, _) in env.to_ledger_snapshot().ledger_entries.iter() {
        if let LedgerKey::ContractData(lookup) = &**key {
            if let Some(hash) = contract_hash_of_sc(&lookup.contract) {
                deployed.insert(hash);
            }
        }
    }
    deployed
}

fn contract_hash(env: &Env, address: &Address) -> Option<[u8; 32]> {
    let ScVal::Address(sc_address) = TryFromVal::try_from_val(env, &address.to_val()).ok()? else {
        return None;
    };
    contract_hash_of_sc(&sc_address)
}

fn contract_hash_of_sc(address: &ScAddress) -> Option<[u8; 32]> {
    let ScAddress::Contract(id) = address else {
        return None;
    };
    Some(id.0 .0)
}

/// Recursively collect every address embedded in a ledger value.
fn collect_addresses(env: &Env, value: &ScVal, out: &mut Vec<Address>) {
    match value {
        ScVal::Address(address) => {
            if let Ok(address) = Address::try_from_val(env, address) {
                out.push(address);
            }
        }
        ScVal::Vec(Some(items)) => {
            for item in items.iter() {
                collect_addresses(env, item, out);
            }
        }
        ScVal::Map(Some(entries)) => {
            for entry in entries.iter() {
                collect_addresses(env, &entry.key, out);
                collect_addresses(env, &entry.val, out);
            }
        }
        _ => {}
    }
}

fn render_key(value: &ScVal) -> String {
    match value {
        ScVal::Symbol(symbol) => String::from_utf8_lossy(symbol.as_slice()).into_owned(),
        ScVal::String(text) => String::from_utf8_lossy(text.as_slice()).into_owned(),
        other => format!("{other:?}"),
    }
}

fn id(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn escape(text: &str) -> String {
    text.replace('"', "'")
}

#[cfg(test)]
mod tests {
    use super::TopologyValidator;
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env, Map,
    };

    #[contract]
    pub struct Pool;

    #[contractimpl]
    impl Pool {
        /// Wires the pool to its token and oracle.
        pub fn initialize(env: Env, token: Address, oracle: Address) {
            env.storage()
                .instance()
                .set(&symbol_short!("token"), &token);
            env.storage()
                .instance()
                .set(&symbol_short!("oracle"), &oracle);
        }

        /// Stores the dependency deeper in a map, where the validator has to
        /// recurse rather than look at top-level keys only.
        pub fn initialize_nested(env: Env, token: Address, oracle: Address) {
            let mut deps = Map::new(&env);
            deps.set(symbol_short!("token"), token);
            deps.set(symbol_short!("oracle"), oracle);
            env.storage().instance().set(&symbol_short!("deps"), &deps);
        }
    }

    #[contract]
    pub struct Vault;

    #[contractimpl]
    impl Vault {
        pub fn initialize(env: Env, pool: Address) {
            env.storage().instance().set(&symbol_short!("pool"), &pool);
        }
    }

    struct Deployed {
        env: Env,
        token: Address,
        oracle: Address,
        pool: Address,
        vault: Address,
    }

    fn deploy() -> Deployed {
        let env = Env::default();
        env.mock_all_auths();
        let issuer = Address::generate(&env);
        let token = env.register_stellar_asset_contract_v2(issuer).address();
        let oracle = env.register(Pool, ());
        let pool = env.register(Pool, ());
        let vault = env.register(Vault, ());
        Deployed {
            env,
            token,
            oracle,
            pool,
            vault,
        }
    }

    fn wired_validator(deployed: &Deployed) -> TopologyValidator<'_> {
        let mut topology = TopologyValidator::new(&deployed.env);
        topology
            .register_contract("token", deployed.token.clone())
            .register_contract("oracle", deployed.oracle.clone())
            .register_contract("pool", deployed.pool.clone())
            .register_contract("vault", deployed.vault.clone());
        topology
    }

    #[test]
    fn a_fully_wired_topology_validates_and_renders() {
        let deployed = deploy();
        PoolClient::new(&deployed.env, &deployed.pool)
            .initialize(&deployed.token, &deployed.oracle);
        VaultClient::new(&deployed.env, &deployed.vault).initialize(&deployed.pool);

        let mut topology = wired_validator(&deployed);
        topology
            .require_link("pool", "token")
            .require_link("pool", "oracle")
            .require_link("vault", "pool");

        let report = topology.validate();
        assert!(report.is_valid(), "{}", report.render_topology());
        assert_eq!(report.edges.len(), 3);
        assert_eq!(report.dangling_references.len(), 0);

        let mermaid = report.to_mermaid();
        assert!(mermaid.starts_with("flowchart TD"));
        assert!(mermaid.contains("pool --> token"));
        assert!(mermaid.contains("pool --> oracle"));
        assert!(mermaid.contains("vault --> pool"));
        assert!(report.render_topology().contains("topology valid"));
    }

    #[test]
    fn the_validator_recurses_into_map_values() {
        let deployed = deploy();
        PoolClient::new(&deployed.env, &deployed.pool)
            .initialize_nested(&deployed.token, &deployed.oracle);

        let mut topology = wired_validator(&deployed);
        topology
            .require_link("pool", "token")
            .require_link("pool", "oracle");

        let report = topology.validate();
        assert!(report.is_valid(), "{}", report.render_topology());
        assert_eq!(report.edges.len(), 2);
    }

    #[test]
    fn a_missing_address_is_an_uninitialized_link() {
        let deployed = deploy();
        // Only the token is wired; the oracle was never stored.
        PoolClient::new(&deployed.env, &deployed.pool).initialize(&deployed.token, &deployed.token);

        let mut topology = wired_validator(&deployed);
        topology
            .require_link("pool", "token")
            .require_link("pool", "oracle");

        let report = topology.validate();
        assert!(!report.is_valid());
        assert_eq!(report.edges.len(), 1);
        assert_eq!(report.uninitialized_links.len(), 1);
        assert_eq!(report.uninitialized_links[0].link.to, "oracle");
        assert!(report
            .render_topology()
            .contains("uninitialized link `pool` -> `oracle`"));
    }

    #[test]
    fn an_undeployed_dependency_is_dangling() {
        let deployed = deploy();
        let ghost = Address::generate(&deployed.env);
        PoolClient::new(&deployed.env, &deployed.pool).initialize(&deployed.token, &ghost);

        let mut topology = wired_validator(&deployed);
        topology
            .require_link("pool", "token")
            .require_link("pool", "oracle");

        let report = topology.validate();
        assert!(!report.is_valid());
        let dangling = report
            .dangling_references
            .iter()
            .find(|dangling| dangling.reference.key == "oracle")
            .expect("the ghost oracle must be reported as dangling");
        assert!(dangling.reason.contains("never deployed"));
        assert!(report.to_mermaid().contains("dangling"));
    }

    #[test]
    fn a_reference_to_an_unregistered_but_deployed_contract_is_flagged() {
        let deployed = deploy();
        PoolClient::new(&deployed.env, &deployed.pool)
            .initialize(&deployed.token, &deployed.oracle);

        // The oracle is deployed but not part of the declared graph.
        let mut topology = TopologyValidator::new(&deployed.env);
        topology
            .register_contract("token", deployed.token.clone())
            .register_contract("pool", deployed.pool.clone());
        topology.require_link("pool", "token");

        let report = topology.validate();
        assert!(!report.is_valid());
        assert!(report
            .dangling_references
            .iter()
            .any(|dangling| dangling.reason.contains("missing from the topology")));
    }

    #[test]
    fn an_unknown_link_endpoint_is_reported() {
        let deployed = deploy();
        let mut topology = wired_validator(&deployed);
        topology.require_link("pool", "keeper");

        let report = topology.validate();
        assert!(!report.is_valid());
        assert_eq!(report.unknown_links.len(), 1);
        assert!(report
            .render_topology()
            .contains("unknown link `pool` -> `keeper`"));
    }

    #[test]
    fn an_undeployed_registered_contract_is_inactive() {
        let deployed = deploy();
        let mut topology = TopologyValidator::new(&deployed.env);
        topology.register_contract("ghost", Address::generate(&deployed.env));

        let report = topology.validate();
        assert!(!report.is_valid());
        assert_eq!(report.inactive_contracts.len(), 1);
        assert!(report.to_mermaid().contains("(inactive)"));
    }

    #[test]
    #[should_panic(expected = "Protocol topology is not fully wired")]
    fn assert_valid_panics_with_the_full_report() {
        let deployed = deploy();
        let mut topology = wired_validator(&deployed);
        topology.require_link("pool", "token");
        topology.assert_valid();
    }

    #[test]
    fn re_registering_a_name_replaces_its_address() {
        let deployed = deploy();
        let replacement = Address::generate(&deployed.env);
        let mut topology = TopologyValidator::new(&deployed.env);
        topology.register_contract("oracle", deployed.oracle.clone());
        topology.register_contract("oracle", replacement.clone());
        assert_eq!(topology.contracts().len(), 1);
        assert_eq!(topology.contracts()[0].address, replacement);
    }

    #[test]
    fn requiring_the_same_link_twice_keeps_one_edge() {
        let deployed = deploy();
        PoolClient::new(&deployed.env, &deployed.pool)
            .initialize(&deployed.token, &deployed.oracle);
        let mut topology = wired_validator(&deployed);
        topology
            .require_link("pool", "token")
            .require_link("pool", "token");

        let report = topology.validate();
        assert!(report.is_valid(), "{}", report.render_topology());
        assert_eq!(report.edges.len(), 1);
    }
}
