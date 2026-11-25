//! Multi-contract protocol registry and upgrade proxy test harness.
//!
//! Production DeFi protocols typically rely on a service-locator address registry
//! and proxy contracts that route calls to active implementation contracts. Testing
//! dynamic service discovery, protocol-wide dependency injection, and live address
//! swaps requires coordinating interconnected contracts on [`Env`].
//!
//! [`ProtocolRegistryHarness`] manages the directory of registered protocol services
//! (`Router`, `Treasury`, `Oracle`, `Vault`), deploys on-chain registry and proxy contracts,
//! simulates protocol upgrades by swapping implementation addresses, and validates that
//! consumer contracts seamlessly resolve updated services.
//!
//! # Quick Start
//!
//! ```rust
//! use soroban_sdk::testutils::Address as _;
//! use soroban_sdk::{Address, Env};
//! use soroban_testkit_fixtures::{ProtocolRegistryHarness, ServiceRole};
//!
//! let env = Env::default();
//! env.mock_all_auths();
//!
//! let mut harness = ProtocolRegistryHarness::new(&env);
//! let oracle_v1 = Address::generate(&env);
//! harness.register_service(ServiceRole::Oracle, &oracle_v1).unwrap();
//!
//! // Consumer contracts or proxies resolve the active oracle
//! assert_eq!(harness.resolve(&ServiceRole::Oracle).unwrap(), oracle_v1);
//!
//! // Simulate a protocol upgrade: swap Oracle v1 for v2
//! let oracle_v2 = Address::generate(&env);
//! harness.upgrade_service(ServiceRole::Oracle, &oracle_v2).unwrap();
//!
//! assert_eq!(harness.resolve(&ServiceRole::Oracle).unwrap(), oracle_v2);
//! println!("{}", harness.to_mermaid());
//! ```

use std::collections::BTreeMap;
use std::fmt;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, Symbol};

/// Canonical roles for core protocol services in a DeFi ecosystem.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceRole {
    /// Order execution and swap routing contract.
    Router,
    /// Protocol treasury and fee collection contract.
    Treasury,
    /// Price feed or asset valuation oracle.
    Oracle,
    /// Asset custody, collateral, or liquidity vault.
    Vault,
    /// Custom protocol service identified by name.
    Custom(String),
}

impl ServiceRole {
    /// Helper for creating the Router role.
    pub fn router() -> Self {
        ServiceRole::Router
    }

    /// Helper for creating the Treasury role.
    pub fn treasury() -> Self {
        ServiceRole::Treasury
    }

    /// Helper for creating the Oracle role.
    pub fn oracle() -> Self {
        ServiceRole::Oracle
    }

    /// Helper for creating the Vault role.
    pub fn vault() -> Self {
        ServiceRole::Vault
    }

    /// Helper for creating a custom service role.
    pub fn custom(name: impl Into<String>) -> Self {
        ServiceRole::Custom(name.into())
    }

    /// Return the string identifier for this service role.
    pub fn as_str(&self) -> &str {
        match self {
            ServiceRole::Router => "router",
            ServiceRole::Treasury => "treasury",
            ServiceRole::Oracle => "oracle",
            ServiceRole::Vault => "vault",
            ServiceRole::Custom(name) => name.as_str(),
        }
    }

    /// Convert this role into an on-chain Soroban [`Symbol`].
    pub fn to_symbol(&self, env: &Env) -> Symbol {
        match self {
            ServiceRole::Router => symbol_short!("router"),
            ServiceRole::Treasury => symbol_short!("treasury"),
            ServiceRole::Oracle => symbol_short!("oracle"),
            ServiceRole::Vault => symbol_short!("vault"),
            ServiceRole::Custom(name) => Symbol::new(env, name.as_str()),
        }
    }
}

impl fmt::Display for ServiceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&str> for ServiceRole {
    fn from(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "router" => ServiceRole::Router,
            "treasury" => ServiceRole::Treasury,
            "oracle" => ServiceRole::Oracle,
            "vault" => ServiceRole::Vault,
            other => ServiceRole::Custom(other.to_string()),
        }
    }
}

impl From<String> for ServiceRole {
    fn from(s: String) -> Self {
        ServiceRole::from(s.as_str())
    }
}

/// Detailed information for a resolution mismatch failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionMismatchDetails {
    pub consumer: String,
    pub role: String,
    pub expected: Address,
    pub actual: Address,
}

/// Errors raised by the [`ProtocolRegistryHarness`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolRegistryError {
    /// Requested service role is not currently registered.
    ServiceNotRegistered(String),
    /// Service role is already registered; use `upgrade_service` instead.
    ServiceAlreadyRegistered(String),
    /// A consumer with the specified name is already registered.
    ConsumerAlreadyRegistered(String),
    /// Consumer contract not found.
    ConsumerNotFound(String),
    /// Resolved address does not match expected address for a consumer dependency.
    ResolutionMismatch(Box<ResolutionMismatchDetails>),
    /// Attempted to upgrade to a version lower than or equal to current version.
    VersionRegression {
        role: String,
        current_version: u32,
        requested_version: u32,
    },
}

impl fmt::Display for ProtocolRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolRegistryError::ServiceNotRegistered(role) => {
                write!(
                    f,
                    "Service '{role}' is not registered in the protocol registry"
                )
            }
            ProtocolRegistryError::ServiceAlreadyRegistered(role) => {
                write!(
                    f,
                    "Service '{role}' is already registered (use upgrade_service to swap addresses)"
                )
            }
            ProtocolRegistryError::ConsumerAlreadyRegistered(name) => {
                write!(f, "Consumer contract '{name}' is already registered")
            }
            ProtocolRegistryError::ConsumerNotFound(name) => {
                write!(f, "Consumer contract '{name}' was not found in harness")
            }
            ProtocolRegistryError::ResolutionMismatch(details) => {
                write!(
                    f,
                    "Consumer '{}' resolved role '{}' to '{:?}', expected '{:?}'",
                    details.consumer, details.role, details.actual, details.expected
                )
            }
            ProtocolRegistryError::VersionRegression {
                role,
                current_version,
                requested_version,
            } => {
                write!(
                    f,
                    "Version regression for service '{role}': requested v{requested_version} <= current v{current_version}"
                )
            }
        }
    }
}

impl std::error::Error for ProtocolRegistryError {}

/// Record of an upgrade event for a protocol service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceUpgradeRecord {
    /// Role that was upgraded.
    pub role: ServiceRole,
    /// Previous implementation address.
    pub previous_address: Address,
    /// New implementation address.
    pub new_address: Address,
    /// Version before upgrade.
    pub from_version: u32,
    /// Version after upgrade.
    pub to_version: u32,
    /// Ledger timestamp when the upgrade occurred.
    pub timestamp: u64,
    /// Optional description or rationale for the upgrade.
    pub note: Option<String>,
}

/// Metadata and history for a registered protocol service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRecord {
    /// Role identifier.
    pub role: ServiceRole,
    /// Active implementation contract address.
    pub current_address: Address,
    /// Current schema / implementation version (starts at 1).
    pub version: u32,
    /// Chronological log of upgrades for this service.
    pub history: Vec<ServiceUpgradeRecord>,
}

/// Dependent consumer contract linked to one or more protocol services.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumerContract {
    /// Logical name of the consumer (e.g. `"LendingPool"` or `"LiquidityRouter"`).
    pub name: String,
    /// On-chain contract address of the consumer.
    pub address: Address,
    /// Service roles this consumer depends on.
    pub dependencies: Vec<ServiceRole>,
}

/// Chronological protocol upgrade log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolUpgradeLogEntry {
    /// Ordinal index of the upgrade.
    pub step: usize,
    /// Service upgrade details.
    pub record: ServiceUpgradeRecord,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
enum RegistryDataKey {
    Admin,
    Service(Symbol),
}

/// On-chain Soroban contract implementing the service-locator address registry.
#[contract]
pub struct ProtocolRegistryContract;

#[contractimpl]
impl ProtocolRegistryContract {
    /// Initialize the registry contract with an administrator address.
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        if env.storage().instance().has(&RegistryDataKey::Admin) {
            panic!("registry already initialized");
        }
        env.storage()
            .instance()
            .set(&RegistryDataKey::Admin, &admin);
    }

    /// Register or update a service address. Requires admin authorization.
    pub fn set_service(env: Env, role: Symbol, address: Address) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&RegistryDataKey::Admin)
            .expect("registry not initialized");
        admin.require_auth();
        env.storage()
            .instance()
            .set(&RegistryDataKey::Service(role), &address);
    }

    /// Look up the active implementation address for a service role.
    pub fn get_service(env: Env, role: Symbol) -> Address {
        env.storage()
            .instance()
            .get(&RegistryDataKey::Service(role.clone()))
            .unwrap_or_else(|| panic!("service '{:?}' not found in registry", role))
    }

    /// Check if a service role is registered.
    pub fn has_service(env: Env, role: Symbol) -> bool {
        env.storage()
            .instance()
            .has(&RegistryDataKey::Service(role))
    }

    /// Remove a registered service. Requires admin authorization.
    pub fn remove_service(env: Env, role: Symbol) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&RegistryDataKey::Admin)
            .expect("registry not initialized");
        admin.require_auth();
        env.storage()
            .instance()
            .remove(&RegistryDataKey::Service(role));
    }
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
enum ProxyDataKey {
    Registry,
    Role,
}

/// Protocol upgrade proxy that dynamically delegates / resolves implementation addresses via the registry.
#[contract]
pub struct UpgradeProxyContract;

#[contractimpl]
impl UpgradeProxyContract {
    /// Initialize the proxy with target registry and service role.
    pub fn initialize(env: Env, registry: Address, role: Symbol) {
        if env.storage().instance().has(&ProxyDataKey::Registry) {
            panic!("proxy already initialized");
        }
        env.storage()
            .instance()
            .set(&ProxyDataKey::Registry, &registry);
        env.storage().instance().set(&ProxyDataKey::Role, &role);
    }

    /// Return the connected registry contract address.
    pub fn get_registry(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&ProxyDataKey::Registry)
            .expect("proxy not initialized")
    }

    /// Return the assigned service role symbol.
    pub fn get_role(env: Env) -> Symbol {
        env.storage()
            .instance()
            .get(&ProxyDataKey::Role)
            .expect("proxy not initialized")
    }

    /// Resolve the active implementation address by querying the connected registry.
    pub fn resolve_implementation(env: Env) -> Address {
        let registry: Address = env
            .storage()
            .instance()
            .get(&ProxyDataKey::Registry)
            .expect("proxy not initialized");
        let role: Symbol = env
            .storage()
            .instance()
            .get(&ProxyDataKey::Role)
            .expect("proxy not initialized");
        let client = ProtocolRegistryContractClient::new(&env, &registry);
        client.get_service(&role)
    }
}

/// Multi-contract protocol registry and upgrade test harness.
///
/// Orchestrates protocol topologies, manages registered services, handles proxy deployment,
/// and tracks upgrade histories.
#[derive(Clone)]
pub struct ProtocolRegistryHarness {
    env: Env,
    admin: Address,
    registry_address: Address,
    services: BTreeMap<ServiceRole, ServiceRecord>,
    consumers: BTreeMap<String, ConsumerContract>,
    upgrade_history: Vec<ProtocolUpgradeLogEntry>,
}

impl fmt::Debug for ProtocolRegistryHarness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProtocolRegistryHarness")
            .field("admin", &self.admin)
            .field("registry_address", &self.registry_address)
            .field("services", &self.services)
            .field("consumers", &self.consumers)
            .field("upgrade_history", &self.upgrade_history)
            .finish()
    }
}

impl ProtocolRegistryHarness {
    /// Deploy a new [`ProtocolRegistryContract`] in `env` with an auto-generated admin.
    pub fn new(env: &Env) -> Self {
        let admin = Address::generate(env);
        Self::with_admin(env, &admin)
    }

    /// Deploy a new [`ProtocolRegistryContract`] in `env` with the specified admin address.
    pub fn with_admin(env: &Env, admin: &Address) -> Self {
        let registry_address = env.register(ProtocolRegistryContract, ());
        let client = ProtocolRegistryContractClient::new(env, &registry_address);
        client.initialize(admin);

        Self {
            env: env.clone(),
            admin: admin.clone(),
            registry_address,
            services: BTreeMap::new(),
            consumers: BTreeMap::new(),
            upgrade_history: Vec::new(),
        }
    }

    /// Return the on-chain address of the central [`ProtocolRegistryContract`].
    pub fn registry_address(&self) -> &Address {
        &self.registry_address
    }

    /// Return the admin address authorized to update registry entries.
    pub fn admin(&self) -> &Address {
        &self.admin
    }

    /// Return the underlying Soroban test environment.
    pub fn env(&self) -> &Env {
        &self.env
    }

    /// Register a core service contract (starts at version 1).
    pub fn register_service(
        &mut self,
        role: ServiceRole,
        address: &Address,
    ) -> Result<&mut Self, ProtocolRegistryError> {
        self.register_service_with_version(role, address, 1)
    }

    /// Register a core service contract with an explicit initial version.
    pub fn register_service_with_version(
        &mut self,
        role: ServiceRole,
        address: &Address,
        version: u32,
    ) -> Result<&mut Self, ProtocolRegistryError> {
        if self.services.contains_key(&role) {
            return Err(ProtocolRegistryError::ServiceAlreadyRegistered(
                role.to_string(),
            ));
        }

        let symbol = role.to_symbol(&self.env);
        let client = ProtocolRegistryContractClient::new(&self.env, &self.registry_address);
        client.set_service(&symbol, address);

        self.services.insert(
            role.clone(),
            ServiceRecord {
                role,
                current_address: address.clone(),
                version,
                history: Vec::new(),
            },
        );

        Ok(self)
    }

    /// Upgrade a service to a new implementation address, incrementing its version by 1.
    pub fn upgrade_service(
        &mut self,
        role: ServiceRole,
        new_address: &Address,
    ) -> Result<ServiceUpgradeRecord, ProtocolRegistryError> {
        let current_version = self
            .services
            .get(&role)
            .map(|s| s.version)
            .ok_or_else(|| ProtocolRegistryError::ServiceNotRegistered(role.to_string()))?;

        self.upgrade_service_with_version(role, new_address, current_version + 1, None::<String>)
    }

    /// Upgrade a service to an explicit target version with an optional upgrade note.
    pub fn upgrade_service_with_version(
        &mut self,
        role: ServiceRole,
        new_address: &Address,
        new_version: u32,
        note: Option<impl Into<String>>,
    ) -> Result<ServiceUpgradeRecord, ProtocolRegistryError> {
        let current_state = self
            .services
            .get_mut(&role)
            .ok_or_else(|| ProtocolRegistryError::ServiceNotRegistered(role.to_string()))?;

        if new_version <= current_state.version {
            return Err(ProtocolRegistryError::VersionRegression {
                role: role.to_string(),
                current_version: current_state.version,
                requested_version: new_version,
            });
        }

        let previous_address = current_state.current_address.clone();
        let from_version = current_state.version;
        let timestamp = self.env.ledger().timestamp();
        let note_str = note.map(Into::into);

        let upgrade_record = ServiceUpgradeRecord {
            role: role.clone(),
            previous_address,
            new_address: new_address.clone(),
            from_version,
            to_version: new_version,
            timestamp,
            note: note_str,
        };

        // Update on-chain registry contract
        let symbol = role.to_symbol(&self.env);
        let client = ProtocolRegistryContractClient::new(&self.env, &self.registry_address);
        client.set_service(&symbol, new_address);

        // Update in-memory state
        current_state.current_address = new_address.clone();
        current_state.version = new_version;
        current_state.history.push(upgrade_record.clone());

        let step = self.upgrade_history.len() + 1;
        self.upgrade_history.push(ProtocolUpgradeLogEntry {
            step,
            record: upgrade_record.clone(),
        });

        Ok(upgrade_record)
    }

    /// Deploy an [`UpgradeProxyContract`] configured to dynamically resolve `role`.
    pub fn deploy_proxy(&self, role: ServiceRole) -> Address {
        let proxy_address = self.env.register(UpgradeProxyContract, ());
        let proxy_client = UpgradeProxyContractClient::new(&self.env, &proxy_address);
        proxy_client.initialize(&self.registry_address, &role.to_symbol(&self.env));
        proxy_address
    }

    /// Resolve the current active address of a service role through the registry.
    pub fn resolve(&self, role: &ServiceRole) -> Result<Address, ProtocolRegistryError> {
        let client = ProtocolRegistryContractClient::new(&self.env, &self.registry_address);
        let symbol = role.to_symbol(&self.env);
        if !client.has_service(&symbol) {
            return Err(ProtocolRegistryError::ServiceNotRegistered(
                role.to_string(),
            ));
        }
        Ok(client.get_service(&symbol))
    }

    /// Resolve the implementation address via an [`UpgradeProxyContract`].
    pub fn resolve_from_proxy(&self, proxy_address: &Address) -> Address {
        let client = UpgradeProxyContractClient::new(&self.env, proxy_address);
        client.resolve_implementation()
    }

    /// Register a consumer contract and its declared service dependencies.
    pub fn register_consumer(
        &mut self,
        name: impl Into<String>,
        address: &Address,
        dependencies: impl IntoIterator<Item = ServiceRole>,
    ) -> Result<&mut Self, ProtocolRegistryError> {
        let name_str = name.into();
        if self.consumers.contains_key(&name_str) {
            return Err(ProtocolRegistryError::ConsumerAlreadyRegistered(name_str));
        }

        self.consumers.insert(
            name_str.clone(),
            ConsumerContract {
                name: name_str,
                address: address.clone(),
                dependencies: dependencies.into_iter().collect(),
            },
        );

        Ok(self)
    }

    /// Retrieve a registered service record.
    pub fn service(&self, role: &ServiceRole) -> Option<&ServiceRecord> {
        self.services.get(role)
    }

    /// Retrieve all registered services.
    pub fn services(&self) -> &BTreeMap<ServiceRole, ServiceRecord> {
        &self.services
    }

    /// Retrieve a consumer contract record.
    pub fn consumer(&self, name: &str) -> Option<&ConsumerContract> {
        self.consumers.get(name)
    }

    /// Retrieve all registered consumer contracts.
    pub fn consumers(&self) -> &BTreeMap<String, ConsumerContract> {
        &self.consumers
    }

    /// Return the full chronicle of all protocol upgrades executed through this harness.
    pub fn upgrade_history(&self) -> &[ProtocolUpgradeLogEntry] {
        &self.upgrade_history
    }

    /// Verify that a consumer contract resolves the expected address for a specific role.
    pub fn verify_consumer_resolution(
        &self,
        consumer_name: &str,
        role: &ServiceRole,
        expected_address: &Address,
    ) -> Result<(), ProtocolRegistryError> {
        let consumer = self
            .consumers
            .get(consumer_name)
            .ok_or_else(|| ProtocolRegistryError::ConsumerNotFound(consumer_name.to_string()))?;

        if !consumer.dependencies.contains(role) {
            return Err(ProtocolRegistryError::ServiceNotRegistered(format!(
                "Consumer '{consumer_name}' does not declare dependency on '{role}'"
            )));
        }

        let actual = self.resolve(role)?;
        if &actual != expected_address {
            return Err(ProtocolRegistryError::ResolutionMismatch(Box::new(
                ResolutionMismatchDetails {
                    consumer: consumer_name.to_string(),
                    role: role.to_string(),
                    expected: expected_address.clone(),
                    actual,
                },
            )));
        }

        Ok(())
    }

    /// Verify that all declared dependencies of all registered consumers are currently registered.
    pub fn verify_all_consumers(&self) -> Result<(), ProtocolRegistryError> {
        for consumer in self.consumers.values() {
            for role in &consumer.dependencies {
                self.resolve(role)?;
            }
        }
        Ok(())
    }

    /// Generate a Mermaid diagram visualizing the protocol topology, services, and consumers.
    pub fn to_mermaid(&self) -> String {
        let mut mermaid = String::new();
        mermaid.push_str("flowchart TD\n");
        mermaid.push_str(&format!(
            "  Registry[\"Registry<br/><code>{:?}</code>\"]\n",
            self.registry_address
        ));

        if !self.services.is_empty() {
            mermaid.push_str("  subgraph Services[\"Protocol Services\"]\n");
            for (role, service) in &self.services {
                let role_id = role.as_str().to_uppercase();
                mermaid.push_str(&format!(
                    "    {}[\"{} (v{})<br/><code>{:?}</code>\"]\n",
                    role_id, role, service.version, service.current_address
                ));
            }
            mermaid.push_str("  end\n");

            for role in self.services.keys() {
                let role_id = role.as_str().to_uppercase();
                mermaid.push_str(&format!("  Registry --> {}\n", role_id));
            }
        }

        if !self.consumers.is_empty() {
            mermaid.push_str("  subgraph Consumers[\"Dependent Consumers\"]\n");
            for (idx, consumer) in self.consumers.values().enumerate() {
                let cid = format!("C_{idx}");
                mermaid.push_str(&format!(
                    "    {}[\"{}<br/><code>{:?}</code>\"]\n",
                    cid, consumer.name, consumer.address
                ));
            }
            mermaid.push_str("  end\n");

            for (idx, consumer) in self.consumers.values().enumerate() {
                let cid = format!("C_{idx}");
                for dep in &consumer.dependencies {
                    let role_id = dep.as_str().to_uppercase();
                    mermaid.push_str(&format!("    {} -. resolves .-> {}\n", cid, role_id));
                }
            }
        }

        mermaid
    }

    /// Render an ASCII / text summary of the protocol topology.
    pub fn render_topology(&self) -> String {
        let mut out = String::new();
        out.push_str(
            "================================================================================\n",
        );
        out.push_str("PROTOCOL REGISTRY TOPOLOGY\n");
        out.push_str(
            "================================================================================\n",
        );
        out.push_str(&format!("Registry Address: {:?}\n", self.registry_address));
        out.push_str(&format!("Admin Address:    {:?}\n", self.admin));
        out.push_str(&format!("Services Registered: {}\n", self.services.len()));

        for (role, record) in &self.services {
            out.push_str(&format!(
                "  - {:<12} [v{}] -> {:?}",
                role.as_str(),
                record.version,
                record.current_address
            ));
            if let Some(last_upgrade) = record.history.last() {
                out.push_str(&format!(" (upgraded from v{})", last_upgrade.from_version));
            }
            out.push('\n');
        }

        out.push_str(&format!("Consumers Connected: {}\n", self.consumers.len()));
        for consumer in self.consumers.values() {
            let deps = consumer
                .dependencies
                .iter()
                .map(|r| r.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "  * {:<16} ({:?}) -> depends on: [{deps}]\n",
                consumer.name, consumer.address
            ));
        }

        if !self.upgrade_history.is_empty() {
            out.push_str("--------------------------------------------------------------------------------\n");
            out.push_str("Upgrade Log:\n");
            for entry in &self.upgrade_history {
                out.push_str(&format!(
                    "  [Step {}] {} upgraded: v{} -> v{}\n",
                    entry.step,
                    entry.record.role,
                    entry.record.from_version,
                    entry.record.to_version,
                ));
            }
        }

        out.push_str(
            "================================================================================\n",
        );
        out
    }

    /// Format this topology as a complete Markdown document.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Protocol Architecture & Service Topology\n\n");
        md.push_str(&format!("- **Registry:** `{:?}`\n", self.registry_address));
        md.push_str(&format!("- **Admin:** `{:?}`\n\n", self.admin));

        md.push_str("## Service Directory\n\n");
        md.push_str("| Service Role | Current Address | Version | Total Upgrades |\n");
        md.push_str("| :--- | :--- | :---: | :---: |\n");
        for (role, service) in &self.services {
            md.push_str(&format!(
                "| `{}` | `{:?}` | v{} | {} |\n",
                role,
                service.current_address,
                service.version,
                service.history.len()
            ));
        }
        md.push('\n');

        if !self.consumers.is_empty() {
            md.push_str("## Dependent Consumers\n\n");
            md.push_str("| Consumer Name | Address | Declared Dependencies |\n");
            md.push_str("| :--- | :--- | :--- |\n");
            for consumer in self.consumers.values() {
                let deps = consumer
                    .dependencies
                    .iter()
                    .map(|d| format!("`{d}`"))
                    .collect::<Vec<_>>()
                    .join(", ");
                md.push_str(&format!(
                    "| **{}** | `{:?}` | {} |\n",
                    consumer.name, consumer.address, deps
                ));
            }
            md.push('\n');
        }

        md.push_str("## Topology Diagram\n\n");
        md.push_str("```mermaid\n");
        md.push_str(&self.to_mermaid());
        md.push_str("```\n\n");

        if !self.upgrade_history.is_empty() {
            md.push_str("## Upgrade History\n\n");
            md.push_str("| Step | Role | From | To | Ledger Timestamp | Note |\n");
            md.push_str("| :---: | :--- | :---: | :---: | :---: | :--- |\n");
            for entry in &self.upgrade_history {
                md.push_str(&format!(
                    "| {} | `{}` | v{} | v{} | {} | {} |\n",
                    entry.step,
                    entry.record.role,
                    entry.record.from_version,
                    entry.record.to_version,
                    entry.record.timestamp,
                    entry.record.note.as_deref().unwrap_or("-")
                ));
            }
            md.push('\n');
        }

        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[contract]
    pub struct MockServiceContract;

    #[contractimpl]
    impl MockServiceContract {
        pub fn get_value(_env: Env) -> u32 {
            100
        }
    }

    #[contract]
    pub struct MockServiceV2Contract;

    #[contractimpl]
    impl MockServiceV2Contract {
        pub fn get_value(_env: Env) -> u32 {
            200
        }
    }

    #[contract]
    pub struct MockConsumerPool;

    #[contractimpl]
    impl MockConsumerPool {
        pub fn initialize(env: Env, registry: Address) {
            env.storage()
                .instance()
                .set(&symbol_short!("reg"), &registry);
        }

        pub fn query_service_value(env: Env, role: Symbol) -> u32 {
            let registry: Address = env.storage().instance().get(&symbol_short!("reg")).unwrap();
            let reg_client = ProtocolRegistryContractClient::new(&env, &registry);
            let service_address = reg_client.get_service(&role);
            let service_client = MockServiceContractClient::new(&env, &service_address);
            service_client.get_value()
        }
    }

    #[test]
    fn test_protocol_registry_initialization_and_roles() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let router_addr = Address::generate(&env);
        let treasury_addr = Address::generate(&env);
        let oracle_addr = Address::generate(&env);
        let vault_addr = Address::generate(&env);

        harness
            .register_service(ServiceRole::Router, &router_addr)
            .unwrap();
        harness
            .register_service(ServiceRole::Treasury, &treasury_addr)
            .unwrap();
        harness
            .register_service(ServiceRole::Oracle, &oracle_addr)
            .unwrap();
        harness
            .register_service(ServiceRole::Vault, &vault_addr)
            .unwrap();

        assert_eq!(harness.services().len(), 4);
        assert_eq!(harness.resolve(&ServiceRole::Router).unwrap(), router_addr);
        assert_eq!(
            harness.resolve(&ServiceRole::Treasury).unwrap(),
            treasury_addr
        );
        assert_eq!(harness.resolve(&ServiceRole::Oracle).unwrap(), oracle_addr);
        assert_eq!(harness.resolve(&ServiceRole::Vault).unwrap(), vault_addr);

        let v_record = harness.service(&ServiceRole::Vault).unwrap();
        assert_eq!(v_record.version, 1);
        assert!(v_record.history.is_empty());
    }

    #[test]
    fn test_duplicate_registration_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let addr1 = Address::generate(&env);
        let addr2 = Address::generate(&env);

        harness
            .register_service(ServiceRole::Router, &addr1)
            .unwrap();
        let err = harness
            .register_service(ServiceRole::Router, &addr2)
            .err()
            .unwrap();

        assert!(matches!(
            err,
            ProtocolRegistryError::ServiceAlreadyRegistered(_)
        ));
    }

    #[test]
    fn test_service_upgrade_and_version_progression() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let oracle_v1 = Address::generate(&env);
        let oracle_v2 = Address::generate(&env);
        let oracle_v3 = Address::generate(&env);

        harness
            .register_service(ServiceRole::Oracle, &oracle_v1)
            .unwrap();

        // Upgrade v1 -> v2
        let record1 = harness
            .upgrade_service(ServiceRole::Oracle, &oracle_v2)
            .unwrap();
        assert_eq!(record1.from_version, 1);
        assert_eq!(record1.to_version, 2);
        assert_eq!(record1.previous_address, oracle_v1);
        assert_eq!(record1.new_address, oracle_v2);
        assert_eq!(harness.resolve(&ServiceRole::Oracle).unwrap(), oracle_v2);

        // Upgrade v2 -> v3 with explicit note
        let record2 = harness
            .upgrade_service_with_version(
                ServiceRole::Oracle,
                &oracle_v3,
                3,
                Some("Upgraded to decentralized TWAP oracle feed"),
            )
            .unwrap();
        assert_eq!(record2.from_version, 2);
        assert_eq!(record2.to_version, 3);
        assert_eq!(harness.resolve(&ServiceRole::Oracle).unwrap(), oracle_v3);

        assert_eq!(harness.upgrade_history().len(), 2);
        let s_record = harness.service(&ServiceRole::Oracle).unwrap();
        assert_eq!(s_record.version, 3);
        assert_eq!(s_record.history.len(), 2);
    }

    #[test]
    fn test_version_regression_rejected() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let v1 = Address::generate(&env);
        let v2 = Address::generate(&env);

        harness
            .register_service_with_version(ServiceRole::Vault, &v1, 5)
            .unwrap();
        let err = harness
            .upgrade_service_with_version(ServiceRole::Vault, &v2, 4, None::<String>)
            .unwrap_err();

        assert!(matches!(
            err,
            ProtocolRegistryError::VersionRegression { .. }
        ));
    }

    #[test]
    fn test_upgrade_proxy_pattern() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let treasury_v1 = Address::generate(&env);
        let treasury_v2 = Address::generate(&env);

        harness
            .register_service(ServiceRole::Treasury, &treasury_v1)
            .unwrap();

        // Deploy an UpgradeProxyContract bound to the Treasury role
        let proxy_address = harness.deploy_proxy(ServiceRole::Treasury);

        // Proxy resolves v1
        let resolved_v1 = harness.resolve_from_proxy(&proxy_address);
        assert_eq!(resolved_v1, treasury_v1);

        // Upgrade treasury to v2
        harness
            .upgrade_service(ServiceRole::Treasury, &treasury_v2)
            .unwrap();

        // Proxy seamlessly resolves v2
        let resolved_v2 = harness.resolve_from_proxy(&proxy_address);
        assert_eq!(resolved_v2, treasury_v2);
    }

    #[test]
    fn test_consumer_dependency_resolution_and_upgrade_propagation() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);

        // Deploy MockService v1
        let service_v1 = env.register(MockServiceContract, ());
        harness
            .register_service(ServiceRole::Oracle, &service_v1)
            .unwrap();

        // Deploy MockConsumerPool connected to registry
        let consumer_pool = env.register(MockConsumerPool, ());
        let consumer_client = MockConsumerPoolClient::new(&env, &consumer_pool);
        consumer_client.initialize(harness.registry_address());

        // Register consumer in harness with Oracle dependency
        harness
            .register_consumer("LiquidityPool", &consumer_pool, [ServiceRole::Oracle])
            .unwrap();
        harness.verify_all_consumers().unwrap();

        // Consumer executes against Service v1
        let val_v1 = consumer_client.query_service_value(&symbol_short!("oracle"));
        assert_eq!(val_v1, 100);

        // Upgrade Oracle to v2
        let service_v2 = env.register(MockServiceV2Contract, ());
        harness
            .upgrade_service(ServiceRole::Oracle, &service_v2)
            .unwrap();

        // Consumer now resolves Service v2 and executes without changing consumer state
        let val_v2 = consumer_client.query_service_value(&symbol_short!("oracle"));
        assert_eq!(val_v2, 200);

        harness
            .verify_consumer_resolution("LiquidityPool", &ServiceRole::Oracle, &service_v2)
            .unwrap();
    }

    #[test]
    fn test_custom_service_roles() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let custom_role = ServiceRole::custom("liquid_staking_controller");
        let controller_addr = Address::generate(&env);

        harness
            .register_service(custom_role.clone(), &controller_addr)
            .unwrap();

        assert_eq!(harness.resolve(&custom_role).unwrap(), controller_addr);
        assert_eq!(custom_role.as_str(), "liquid_staking_controller");
    }

    #[test]
    fn test_topology_diagram_rendering() {
        let env = Env::default();
        env.mock_all_auths();

        let mut harness = ProtocolRegistryHarness::new(&env);
        let r_addr = Address::generate(&env);
        let t_addr = Address::generate(&env);
        let o_addr = Address::generate(&env);
        let v_addr = Address::generate(&env);

        harness
            .register_service(ServiceRole::Router, &r_addr)
            .unwrap();
        harness
            .register_service(ServiceRole::Treasury, &t_addr)
            .unwrap();
        harness
            .register_service(ServiceRole::Oracle, &o_addr)
            .unwrap();
        harness
            .register_service(ServiceRole::Vault, &v_addr)
            .unwrap();

        let pool_addr = Address::generate(&env);
        harness
            .register_consumer(
                "StakingPool",
                &pool_addr,
                [ServiceRole::Oracle, ServiceRole::Vault],
            )
            .unwrap();

        let mermaid = harness.to_mermaid();
        assert!(mermaid.contains("flowchart TD"));
        assert!(mermaid.contains("ROUTER"));
        assert!(mermaid.contains("TREASURY"));
        assert!(mermaid.contains("ORACLE"));
        assert!(mermaid.contains("VAULT"));
        assert!(mermaid.contains("StakingPool"));
        assert!(mermaid.contains("resolves"));

        let text = harness.render_topology();
        assert!(text.contains("PROTOCOL REGISTRY TOPOLOGY"));
        assert!(text.contains("router"));
        assert!(text.contains("StakingPool"));

        let md = harness.to_markdown();
        assert!(md.contains("# Protocol Architecture & Service Topology"));
        assert!(md.contains("## Service Directory"));
        assert!(md.contains("```mermaid"));
    }
}
