//! Storage key collision and namespace isolation auditing.
//!
//! Contracts assembled from multiple modules or trait implementations can
//! accidentally write to the same bare symbol key (for example `"admin"`).
//! [`NamespaceAuditor`] records the keys written during a test run, detects
//! keys shared by independent modules, and flags bare symbols that are at risk
//! of colliding.

use crate::storage::{inspect_storage, StorageEntry, StorageTier};
use soroban_sdk::{Address, Env};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Separators that indicate a key is namespaced, e.g. `DataKey::Admin`,
/// `ownable:admin`, `vault.admin`, or `vault/admin`.
pub const NAMESPACE_SEPARATORS: [&str; 4] = ["::", ":", ".", "/"];

/// A storage key observed during a test run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRecord {
    /// Logical module or trait implementation that wrote the key.
    pub module: String,
    /// The storage key as a string.
    pub key: String,
    /// Optional storage tier when the key came from a snapshot.
    pub tier: Option<StorageTier>,
}

impl KeyRecord {
    /// Create a record for a module/key pair.
    pub fn new(module: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            key: key.into(),
            tier: None,
        }
    }

    /// Create a record from a storage snapshot entry.
    pub fn from_storage_entry(module: impl Into<String>, entry: &StorageEntry) -> Self {
        Self {
            module: module.into(),
            key: entry.key.clone(),
            tier: Some(entry.tier),
        }
    }

    /// The namespace portion of the key, if it has one.
    ///
    /// `DataKey::Admin` yields `Some("DataKey")`, while a bare `admin` key
    /// yields `None`.
    pub fn namespace(&self) -> Option<&str> {
        namespace_of(&self.key)
    }

    /// Whether the key uses a namespace or typed enum discriminant.
    pub fn is_namespaced(&self) -> bool {
        self.namespace().is_some()
    }

    /// The final key segment after any namespace separator.
    pub fn bare_key(&self) -> &str {
        split_namespace(&self.key).map_or(self.key.as_str(), |(_, rest)| rest)
    }
}

/// A key written by more than one independent module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceCollision {
    /// The colliding key.
    pub key: String,
    /// Every module that wrote the key.
    pub modules: Vec<String>,
}

impl fmt::Display for NamespaceCollision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} written by {}", self.key, self.modules.join(", "))
    }
}

/// The result of auditing a set of storage keys.
#[derive(Debug, Clone, Default)]
pub struct NamespaceReport {
    entries: Vec<KeyRecord>,
    collisions: Vec<NamespaceCollision>,
    un_namespaced: Vec<KeyRecord>,
    modules: BTreeMap<String, Vec<String>>,
}

impl NamespaceReport {
    /// Whether any key is shared between independent modules.
    pub fn has_collisions(&self) -> bool {
        !self.collisions.is_empty()
    }

    /// Keys written by more than one module.
    pub fn collisions(&self) -> &[NamespaceCollision] {
        &self.collisions
    }

    /// Keys without a namespace or typed enum discriminant.
    pub fn un_namespaced_keys(&self) -> &[KeyRecord] {
        &self.un_namespaced
    }

    /// Every recorded key.
    pub fn entries(&self) -> &[KeyRecord] {
        &self.entries
    }

    /// Map of module name to the keys it owns, sorted and deduplicated.
    pub fn modules(&self) -> &BTreeMap<String, Vec<String>> {
        &self.modules
    }

    /// Whether the audit found no collisions and no un-namespaced keys.
    pub fn is_clean(&self) -> bool {
        self.collisions.is_empty() && self.un_namespaced.is_empty()
    }
}

impl fmt::Display for NamespaceReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Storage namespaces:")?;
        if self.modules.is_empty() {
            writeln!(f, "  (none)")?;
        }
        for (module, keys) in &self.modules {
            writeln!(f, "  {module}: {}", keys.join(", "))?;
        }

        writeln!(f, "Collisions:")?;
        if self.collisions.is_empty() {
            writeln!(f, "  (none)")?;
        }
        for collision in &self.collisions {
            writeln!(f, "  {collision}")?;
        }

        writeln!(f, "Un-namespaced keys:")?;
        if self.un_namespaced.is_empty() {
            writeln!(f, "  (none)")?;
        }
        for record in &self.un_namespaced {
            writeln!(f, "  {}: {}", record.module, record.key)?;
        }
        Ok(())
    }
}

/// Records storage keys and produces a [`NamespaceReport`].
#[derive(Debug, Clone, Default)]
pub struct NamespaceAuditor {
    contract_id: Option<Address>,
    records: Vec<KeyRecord>,
}

impl NamespaceAuditor {
    /// Create an auditor for a deployed contract.
    ///
    /// Any keys already available from [`inspect_storage`] are loaded with the
    /// module name `contract`. Record additional module keys with
    /// [`NamespaceAuditor::record`] or [`NamespaceAuditor::record_storage`].
    pub fn new(env: &Env, contract: &Address) -> Self {
        let mut auditor = Self {
            contract_id: Some(contract.clone()),
            records: Vec::new(),
        };
        for entry in inspect_storage(env, contract) {
            auditor
                .records
                .push(KeyRecord::from_storage_entry("contract", &entry));
        }
        auditor
    }

    /// Build an auditor from storage snapshot entries, inferring the module
    /// from each key's namespace and falling back to `contract`.
    pub fn from_entries(entries: &[StorageEntry]) -> Self {
        let records = entries
            .iter()
            .map(|entry| {
                let module = namespace_of(&entry.key).unwrap_or("contract");
                KeyRecord::from_storage_entry(module, entry)
            })
            .collect();
        Self {
            contract_id: None,
            records,
        }
    }

    /// The contract being audited, if one was supplied.
    pub fn contract_id(&self) -> Option<&Address> {
        self.contract_id.as_ref()
    }

    /// Record a key written by a module.
    pub fn record(&mut self, module: impl Into<String>, key: impl Into<String>) -> &mut Self {
        self.records.push(KeyRecord::new(module, key));
        self
    }

    /// Record a key from a storage snapshot entry.
    pub fn record_storage(&mut self, module: impl Into<String>, entry: &StorageEntry) -> &mut Self {
        self.records
            .push(KeyRecord::from_storage_entry(module, entry));
        self
    }

    /// Record many keys for a single module.
    pub fn record_all<I, S>(&mut self, module: impl Into<String>, keys: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let module = module.into();
        for key in keys {
            self.records.push(KeyRecord::new(module.clone(), key));
        }
        self
    }

    /// Every recorded key.
    pub fn records(&self) -> &[KeyRecord] {
        &self.records
    }

    /// Audit the recorded keys.
    pub fn audit(&self) -> NamespaceReport {
        let mut modules: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut un_namespaced = BTreeMap::<(String, String), KeyRecord>::new();

        for record in &self.records {
            modules
                .entry(record.module.clone())
                .or_default()
                .insert(record.key.clone());
            owners
                .entry(record.key.clone())
                .or_default()
                .insert(record.module.clone());
            if !record.is_namespaced() {
                un_namespaced
                    .entry((record.module.clone(), record.key.clone()))
                    .or_insert_with(|| record.clone());
            }
        }

        let collisions = owners
            .into_iter()
            .filter(|(_, modules)| modules.len() > 1)
            .map(|(key, modules)| NamespaceCollision {
                key,
                modules: modules.into_iter().collect(),
            })
            .collect();

        let modules = modules
            .into_iter()
            .map(|(module, keys)| (module, keys.into_iter().collect()))
            .collect();

        NamespaceReport {
            entries: self.records.clone(),
            collisions,
            un_namespaced: un_namespaced.into_values().collect(),
            modules,
        }
    }
}

fn namespace_of(key: &str) -> Option<&str> {
    split_namespace(key).map(|(namespace, _)| namespace)
}

fn split_namespace(key: &str) -> Option<(&str, &str)> {
    let mut best: Option<(usize, &str)> = None;
    for separator in NAMESPACE_SEPARATORS {
        if let Some(index) = key.find(separator) {
            let replace = best.is_none_or(|(current, _)| index < current);
            if replace {
                best = Some((index, separator));
            }
        }
    }

    let (index, separator) = best?;
    let namespace = &key[..index];
    let rest = &key[index + separator.len()..];
    if namespace.is_empty() || rest.is_empty() {
        None
    } else {
        Some((namespace, rest))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, tier: StorageTier) -> StorageEntry {
        StorageEntry {
            key: key.to_string(),
            tier,
            value: String::new(),
            bytes: Vec::new(),
            live_until: None,
        }
    }

    #[test]
    fn namespaced_keys_do_not_collide() {
        let mut auditor = NamespaceAuditor::default();
        auditor
            .record("ownable", "ownable::admin")
            .record("pausable", "pausable::paused");
        let report = auditor.audit();
        assert!(!report.has_collisions());
        assert!(report.un_namespaced_keys().is_empty());
        assert!(report.is_clean());
    }

    #[test]
    fn bare_symbol_collision_is_detected() {
        let mut auditor = NamespaceAuditor::default();
        auditor
            .record("ownable", "admin")
            .record("pausable", "admin");
        let report = auditor.audit();
        assert!(report.has_collisions());
        assert_eq!(report.collisions().len(), 1);
        assert_eq!(report.collisions()[0].key, "admin");
        assert_eq!(report.collisions()[0].modules, vec!["ownable", "pausable"]);
    }

    #[test]
    fn un_namespaced_keys_are_flagged() {
        let mut auditor = NamespaceAuditor::default();
        auditor
            .record("token", "balance")
            .record("token", "DataKey::Supply");
        let report = auditor.audit();
        let flagged = report.un_namespaced_keys();
        assert_eq!(flagged.len(), 1);
        assert_eq!(flagged[0].key, "balance");
    }

    #[test]
    fn typed_enum_discriminants_are_namespaced() {
        let record = KeyRecord::new("token", "DataKey::Admin");
        assert_eq!(record.namespace(), Some("DataKey"));
        assert_eq!(record.bare_key(), "Admin");
        assert!(record.is_namespaced());
    }

    #[test]
    fn namespace_detection_supports_multiple_separators() {
        assert_eq!(namespace_of("ownable:admin"), Some("ownable"));
        assert_eq!(namespace_of("vault.admin"), Some("vault"));
        assert_eq!(namespace_of("vault/admin"), Some("vault"));
        assert_eq!(namespace_of("admin"), None);
        assert_eq!(namespace_of("::admin"), None);
        assert_eq!(namespace_of("vault::"), None);
    }

    #[test]
    fn module_map_groups_and_deduplicates_keys() {
        let mut auditor = NamespaceAuditor::default();
        auditor
            .record("token", "token::admin")
            .record("token", "token::admin")
            .record("token", "token::supply");
        let report = auditor.audit();
        assert_eq!(
            report.modules().get("token").unwrap(),
            &vec!["token::admin".to_string(), "token::supply".to_string()]
        );
    }

    #[test]
    fn shared_namespaced_key_is_a_collision() {
        let mut auditor = NamespaceAuditor::default();
        auditor
            .record("module_a", "shared::state")
            .record("module_b", "shared::state");
        let report = auditor.audit();
        assert!(report.has_collisions());
    }

    #[test]
    fn from_entries_infers_modules_from_namespaces() {
        let entries = vec![
            entry("ownable::admin", StorageTier::Instance),
            entry("pausable::paused", StorageTier::Persistent),
            entry("loose", StorageTier::Temporary),
        ];
        let report = NamespaceAuditor::from_entries(&entries).audit();
        assert!(report.modules().contains_key("ownable"));
        assert!(report.modules().contains_key("pausable"));
        assert!(report.modules().contains_key("contract"));
        assert_eq!(report.un_namespaced_keys().len(), 1);
    }

    #[test]
    fn report_display_lists_modules_and_collisions() {
        let mut auditor = NamespaceAuditor::default();
        auditor
            .record("ownable", "admin")
            .record("pausable", "admin");
        let rendered = auditor.audit().to_string();
        assert!(rendered.contains("ownable: admin"));
        assert!(rendered.contains("admin written by ownable, pausable"));
        assert!(rendered.contains("Un-namespaced keys:"));
    }

    #[test]
    fn empty_auditor_produces_clean_report() {
        let report = NamespaceAuditor::default().audit();
        assert!(report.is_clean());
        assert!(report.entries().is_empty());
        assert!(report.to_string().contains("(none)"));
    }
}
