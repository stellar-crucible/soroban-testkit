use std::collections::BTreeMap;

use soroban_sdk::{
    xdr::{ContractDataDurability, LedgerEntryData, LedgerKey, ScAddress, ScVal},
    Address, Env,
};

/// Where a contract storage entry lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StorageTier {
    Instance,
    Persistent,
    Temporary,
}

impl StorageTier {
    /// Stable label, used in rendered identities and failure messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            StorageTier::Instance => "instance",
            StorageTier::Persistent => "persistent",
            StorageTier::Temporary => "temporary",
        }
    }
}

/// A live storage entry, with key and value rendered to stable strings so two
/// captures of the same state compare equal.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StorageEntry {
    pub key: String,
    pub tier: StorageTier,
    pub value: String,
    /// Ledger sequence the entry dies at; `None` when the entry never expires.
    pub live_until: Option<u32>,
}

impl StorageEntry {
    fn identity(&self) -> (StorageTier, String) {
        (self.tier, self.key.clone())
    }

    /// `tier:key`, the form failure messages quote.
    pub fn qualified(&self) -> String {
        format!("{}:{}", self.tier.as_str(), self.key)
    }
}

/// Every live storage entry of one contract at a point in time.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StorageSnapshot {
    entries: Vec<StorageEntry>,
}

impl StorageSnapshot {
    /// Read `contract`'s live storage out of a test environment.
    ///
    /// A non-contract address (account, malleable) owns no contract data, so it
    /// captures as an empty snapshot rather than failing.
    pub fn capture(env: &Env, contract: &Address) -> Self {
        let Some(wanted) = contract_hash_of(env, contract) else {
            return Self::default();
        };
        let mut found: BTreeMap<(StorageTier, String), StorageEntry> = BTreeMap::new();

        for (key, (entry, live_until)) in env.to_ledger_snapshot().ledger_entries.iter() {
            let LedgerKey::ContractData(lookup) = &**key else {
                continue;
            };
            if contract_hash(&lookup.contract) != Some(wanted) {
                continue;
            };
            let LedgerEntryData::ContractData(stored) = &entry.data else {
                continue;
            };

            match &stored.key {
                // Instance storage is one container entry; unfold it into one
                // entry per key so instance keys read like the other tiers.
                ScVal::LedgerKeyContractInstance => {
                    let ScVal::ContractInstance(instance) = &stored.val else {
                        continue;
                    };
                    let Some(storage) = &instance.storage else {
                        continue;
                    };
                    for item in storage.0.iter() {
                        let key = render(&item.key);
                        found.insert(
                            (StorageTier::Instance, key.clone()),
                            StorageEntry {
                                key,
                                tier: StorageTier::Instance,
                                value: render(&item.val),
                                live_until: *live_until,
                            },
                        );
                    }
                }
                stored_key => {
                    let tier = match stored.durability {
                        ContractDataDurability::Temporary => StorageTier::Temporary,
                        ContractDataDurability::Persistent => StorageTier::Persistent,
                    };
                    let key = render(stored_key);
                    found.insert(
                        (tier, key.clone()),
                        StorageEntry {
                            key,
                            tier,
                            value: render(&stored.val),
                            live_until: *live_until,
                        },
                    );
                }
            }
        }

        Self {
            entries: found.into_values().collect(),
        }
    }

    pub fn entries(&self) -> &[StorageEntry] {
        &self.entries
    }

    pub fn into_entries(self) -> Vec<StorageEntry> {
        self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry with a rendered key, if this snapshot holds one.
    pub fn get(&self, key: &str) -> Option<&StorageEntry> {
        self.entries.iter().find(|entry| entry.key == key)
    }

    pub fn in_tier(&self, tier: StorageTier) -> Vec<&StorageEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.tier == tier)
            .collect()
    }

    /// Keys added, removed and rewritten between this snapshot and a later one.
    pub fn diff(&self, after: &StorageSnapshot) -> StorageDiff {
        let before = index(&self.entries);
        let later = index(&after.entries);

        let added = later
            .iter()
            .filter(|(identity, _)| !before.contains_key(*identity))
            .map(|(_, entry)| (*entry).clone())
            .collect();
        let removed = before
            .iter()
            .filter(|(identity, _)| !later.contains_key(*identity))
            .map(|(_, entry)| (*entry).clone())
            .collect();
        let modified = before
            .iter()
            .filter_map(|(identity, previous)| {
                let current = later.get(identity)?;
                (previous.value != current.value).then(|| StorageChange {
                    key: current.key.clone(),
                    tier: current.tier,
                    before: previous.value.clone(),
                    after: current.value.clone(),
                })
            })
            .collect();

        StorageDiff {
            added,
            removed,
            modified,
        }
    }

    /// Fail unless nothing between the two captures was added, removed or rewritten.
    pub fn assert_unchanged(&self, after: &StorageSnapshot) {
        let diff = self.diff(after);
        assert!(
            diff.is_empty(),
            "Expected storage to be unchanged, but found {} added, {} removed, {} modified: {}",
            diff.added.len(),
            diff.removed.len(),
            diff.modified.len(),
            diff.summary(),
        );
    }

    /// Fail unless `key` appears in `after` and not in this snapshot.
    pub fn assert_entry_added(&self, after: &StorageSnapshot, key: &str) {
        let diff = self.diff(after);
        assert!(
            diff.added.iter().any(|entry| entry.key == key),
            "Expected entry {:?} to be added, but the additions were {:?}",
            key,
            names(&diff.added),
        );
    }

    /// Fail unless `key` appears in this snapshot and not in `after`.
    pub fn assert_entry_removed(&self, after: &StorageSnapshot, key: &str) {
        let diff = self.diff(after);
        assert!(
            diff.removed.iter().any(|entry| entry.key == key),
            "Expected entry {:?} to be removed, but the removals were {:?}",
            key,
            names(&diff.removed),
        );
    }
}

/// The difference between two [`StorageSnapshot`]s.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StorageDiff {
    pub added: Vec<StorageEntry>,
    pub removed: Vec<StorageEntry>,
    pub modified: Vec<StorageChange>,
}

/// One key whose value was rewritten between two captures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageChange {
    pub key: String,
    pub tier: StorageTier,
    pub before: String,
    pub after: String,
}

impl StorageDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }

    pub fn len(&self) -> usize {
        self.added.len() + self.removed.len() + self.modified.len()
    }

    pub fn added(&self) -> &[StorageEntry] {
        &self.added
    }

    pub fn removed(&self) -> &[StorageEntry] {
        &self.removed
    }

    pub fn modified(&self) -> &[StorageChange] {
        &self.modified
    }

    fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(
            self.added
                .iter()
                .map(|entry| format!("+ {}", entry.qualified())),
        );
        parts.extend(
            self.removed
                .iter()
                .map(|entry| format!("- {}", entry.qualified())),
        );
        parts.extend(self.modified.iter().map(|change| {
            format!(
                "~ {}:{} {} -> {}",
                change.tier.as_str(),
                change.key,
                change.before,
                change.after
            )
        }));
        parts.join(", ")
    }
}

/// Flat view of one contract's live storage.
pub fn inspect_storage(env: &Env, contract: &Address) -> Vec<StorageEntry> {
    StorageSnapshot::capture(env, contract).into_entries()
}

fn index(entries: &[StorageEntry]) -> BTreeMap<(StorageTier, String), &StorageEntry> {
    entries
        .iter()
        .map(|entry| (entry.identity(), entry))
        .collect()
}

fn names(entries: &[StorageEntry]) -> Vec<String> {
    entries.iter().map(StorageEntry::qualified).collect()
}

fn contract_hash(address: &ScAddress) -> Option<[u8; 32]> {
    let ScAddress::Contract(id) = address else {
        return None;
    };
    Some(id.0 .0)
}

/// The 32-byte id of a contract address, `None` for any other address type.
fn contract_hash_of(env: &Env, address: &Address) -> Option<[u8; 32]> {
    let ScVal::Address(sc_address) =
        soroban_sdk::TryFromVal::try_from_val(env, &address.to_val()).ok()?
    else {
        return None;
    };
    contract_hash(&sc_address)
}

/// Render an XDR value as a stable, readable string.
fn render(value: &ScVal) -> String {
    match value {
        ScVal::LedgerKeyContractInstance => "instance".to_string(),
        ScVal::Void => "void".to_string(),
        ScVal::Bool(true) => "true".to_string(),
        ScVal::Bool(false) => "false".to_string(),
        ScVal::Symbol(symbol) => String::from_utf8_lossy(symbol.as_slice()).into_owned(),
        ScVal::U32(number) => number.to_string(),
        ScVal::I32(number) => number.to_string(),
        ScVal::U64(number) => number.to_string(),
        ScVal::I64(number) => number.to_string(),
        ScVal::Timepoint(point) => point.0.to_string(),
        ScVal::Duration(duration) => duration.0.to_string(),
        ScVal::Vec(Some(items)) => {
            format!(
                "[{}]",
                items.iter().map(render).collect::<Vec<_>>().join(", ")
            )
        }
        ScVal::Map(Some(entries)) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|entry| format!("{}: {}", render(&entry.key), render(&entry.val)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{inspect_storage, StorageSnapshot, StorageTier};
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env, Map, Symbol,
    };

    #[contract]
    pub struct Store;

    #[contractimpl]
    impl Store {
        pub fn seed(env: Env, admin: Address) {
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "count"), &42u32);
            env.storage()
                .temporary()
                .set(&Symbol::new(&env, "ttl"), &7u32);
            env.storage()
                .instance()
                .set(&symbol_short!("admin"), &admin);
        }

        pub fn bump(env: Env) {
            let key = Symbol::new(&env, "count");
            let current: u32 = env.storage().persistent().get(&key).unwrap();
            env.storage().persistent().set(&key, &(current + 1));
        }

        pub fn erase(env: Env) {
            env.storage()
                .persistent()
                .remove(&Symbol::new(&env, "count"));
        }

        pub fn write_map(env: Env) {
            let mut map = Map::new(&env);
            map.set(symbol_short!("a"), 1u32);
            map.set(symbol_short!("b"), 2u32);
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "map"), &map);
        }
    }

    fn seeded() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(Store, ());
        StoreClient::new(&env, &contract).seed(&Address::generate(&env));
        (env, contract)
    }

    #[test]
    fn capture_finds_every_tier() {
        let (env, contract) = seeded();
        let snapshot = StorageSnapshot::capture(&env, &contract);

        let keys: Vec<&str> = snapshot.entries().iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["admin", "count", "ttl"]);

        assert_eq!(snapshot.get("admin").unwrap().tier, StorageTier::Instance);
        assert_eq!(snapshot.get("count").unwrap().tier, StorageTier::Persistent);
        assert_eq!(snapshot.get("ttl").unwrap().tier, StorageTier::Temporary);
    }

    #[test]
    fn values_are_rendered_readably() {
        let (env, contract) = seeded();
        let snapshot = StorageSnapshot::capture(&env, &contract);

        assert_eq!(snapshot.get("count").unwrap().value, "42");
        assert_eq!(snapshot.get("ttl").unwrap().value, "7");
        assert!(snapshot.get("admin").unwrap().value.starts_with("Address"));
    }

    #[test]
    fn instance_storage_is_unfolded_per_key() {
        let (env, contract) = seeded();
        let snapshot = StorageSnapshot::capture(&env, &contract);

        let instance = snapshot.in_tier(StorageTier::Instance);
        assert_eq!(instance.len(), 1);
        assert_eq!(instance[0].key, "admin");
    }

    #[test]
    fn map_values_keep_their_shape() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).write_map();

        let snapshot = StorageSnapshot::capture(&env, &contract);
        assert_eq!(snapshot.get("map").unwrap().value, "{a: 1, b: 2}");
    }

    #[test]
    fn ttl_is_captured_and_temporary_expires_first() {
        let (env, contract) = seeded();
        let snapshot = StorageSnapshot::capture(&env, &contract);

        let persistent = snapshot.get("count").unwrap().live_until;
        let temporary = snapshot.get("ttl").unwrap().live_until;
        assert!(persistent.is_some());
        assert!(temporary < persistent);
    }

    #[test]
    fn another_contract_storage_is_invisible() {
        let (env, first) = seeded();
        let second = env.register(Store, ());

        assert_eq!(StorageSnapshot::capture(&env, &first).len(), 3);
        assert!(StorageSnapshot::capture(&env, &second).is_empty());
    }

    #[test]
    fn inspect_storage_matches_the_snapshot() {
        let (env, contract) = seeded();
        let snapshot = StorageSnapshot::capture(&env, &contract);

        assert_eq!(
            inspect_storage(&env, &contract),
            snapshot.entries().to_vec()
        );
    }

    #[test]
    fn diff_reports_added_entries_against_an_empty_start() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let before = StorageSnapshot::capture(&env, &contract);

        StoreClient::new(&env, &contract).seed(&Address::generate(&env));
        let after = StorageSnapshot::capture(&env, &contract);

        let diff = before.diff(&after);
        assert_eq!(diff.len(), 3);
        assert!(diff.removed().is_empty() && diff.modified().is_empty());
        assert_eq!(
            diff.added()
                .iter()
                .map(|entry| entry.qualified())
                .collect::<Vec<_>>(),
            vec!["instance:admin", "persistent:count", "temporary:ttl"],
        );
    }

    #[test]
    fn diff_reports_a_rewritten_value() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);

        StoreClient::new(&env, &contract).bump();
        let after = StorageSnapshot::capture(&env, &contract);

        let diff = before.diff(&after);
        assert!(diff.added().is_empty() && diff.removed().is_empty());
        assert_eq!(
            diff.modified(),
            &[super::StorageChange {
                key: "count".to_string(),
                tier: StorageTier::Persistent,
                before: "42".to_string(),
                after: "43".to_string(),
            }]
        );
    }

    #[test]
    fn diff_reports_a_removed_entry() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);

        StoreClient::new(&env, &contract).erase();
        let after = StorageSnapshot::capture(&env, &contract);

        let diff = before.diff(&after);
        assert_eq!(diff.removed().len(), 1);
        assert_eq!(diff.removed()[0].qualified(), "persistent:count");
        assert!(diff.added().is_empty() && diff.modified().is_empty());
    }

    #[test]
    fn unchanged_storage_passes_the_assertions() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        let after = StorageSnapshot::capture(&env, &contract);

        before.assert_unchanged(&after);
    }

    #[test]
    fn entry_added_accepts_a_real_addition() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let before = StorageSnapshot::capture(&env, &contract);

        StoreClient::new(&env, &contract).seed(&Address::generate(&env));
        let after = StorageSnapshot::capture(&env, &contract);

        before.assert_entry_added(&after, "count");
        before.assert_entry_added(&after, "admin");
    }

    #[test]
    fn entry_removed_accepts_a_real_removal() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);

        StoreClient::new(&env, &contract).erase();
        let after = StorageSnapshot::capture(&env, &contract);

        before.assert_entry_removed(&after, "count");
    }

    #[test]
    #[should_panic(expected = "Expected storage to be unchanged, but found 1 added")]
    fn assert_unchanged_reports_the_change() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);

        StoreClient::new(&env, &contract).write_map();
        let after = StorageSnapshot::capture(&env, &contract);

        before.assert_unchanged(&after);
    }

    #[test]
    #[should_panic(expected = "Expected entry \"absent\" to be added")]
    fn assert_entry_added_panics_when_missing() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        let after = StorageSnapshot::capture(&env, &contract);

        before.assert_entry_added(&after, "absent");
    }

    #[test]
    #[should_panic(expected = "Expected entry \"absent\" to be removed")]
    fn assert_entry_removed_panics_when_missing() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        let after = StorageSnapshot::capture(&env, &contract);

        before.assert_entry_removed(&after, "absent");
    }

    #[test]
    fn qualified_entries_carry_their_tier() {
        let (env, contract) = seeded();
        let snapshot = StorageSnapshot::capture(&env, &contract);

        assert_eq!(
            snapshot.get("count").unwrap().qualified(),
            "persistent:count"
        );
        assert_eq!(StorageTier::Temporary.as_str(), "temporary");
    }
}
