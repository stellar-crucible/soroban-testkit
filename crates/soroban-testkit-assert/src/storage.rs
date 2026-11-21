//! Storage assertions.
//!
//! Two families share this module: entry count quotas per tier
//! ([`StorageQuotaMatcher`] and [`assert_storage_tier_count!`]) and assertions
//! that entries are gone ([`assert_storage_empty`] and
//! [`assert_storage_key_deleted`]).
//!
//! The quota matcher enforces exact counts or upper and lower bounds on
//! instance, persistent and temporary storage, and prints a per-tier table
//! when a budget is breached.
//!
//! The deletion assertions prove entries are gone from the ledger rather than
//! merely rewritten: a key holding `0`, an empty `Option::None`, an empty map
//! or any other tombstone value is still a live ledger entry and fails them.
//! Both inspect all three tiers unless one tier is named, and a failure lists
//! every surviving entry with its tier, rendered value and XDR bytes.
//!
//! ```
//! use soroban_testkit_assert::{assert_storage_empty, assert_storage_key_deleted};
//! use soroban_testkit_assert::storage::StorageTier;
//! # use soroban_sdk::{Address, Env};
//!
//! # fn example(env: &Env, contract: &Address) {
//! assert_storage_key_deleted!(env, contract, "count");
//! assert_storage_key_deleted!(env, contract, "count", StorageTier::Persistent);
//! assert_storage_empty!(env, contract);
//! assert_storage_empty!(env, contract, StorageTier::Temporary);
//! # }
//! ```

use std::collections::BTreeMap;
use std::fmt;

use soroban_sdk::{Address, Env};
use soroban_testkit_core::storage::{StorageEntry, StorageSnapshot};

pub use soroban_testkit_core::storage::StorageTier;

/// Quota specification for a storage tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierQuota {
    /// No quota is enforced for this tier.
    Unconstrained,
    /// Exactly `n` entries must be stored in this tier.
    Exact(usize),
    /// At most `n` entries may be stored in this tier.
    Max(usize),
    /// At least `n` entries must be stored in this tier.
    Min(usize),
}

impl TierQuota {
    /// Check whether a measured count satisfies this quota.
    pub fn is_satisfied(&self, count: usize) -> bool {
        match self {
            TierQuota::Unconstrained => true,
            TierQuota::Exact(expected) => count == *expected,
            TierQuota::Max(limit) => count <= *limit,
            TierQuota::Min(limit) => count >= *limit,
        }
    }

    /// Short descriptive label of the quota requirement.
    pub fn description(&self) -> String {
        match self {
            TierQuota::Unconstrained => "unconstrained".to_string(),
            TierQuota::Exact(n) => format!("exact {n}"),
            TierQuota::Max(n) => format!("max {n}"),
            TierQuota::Min(n) => format!("min {n}"),
        }
    }
}

/// Inspection results for a single storage tier.
#[derive(Debug, Clone)]
pub struct TierInspection {
    pub tier: StorageTier,
    pub quota: TierQuota,
    pub count: usize,
    pub keys: Vec<String>,
    pub passed: bool,
}

/// Matcher that validates entry counts in each storage tier against configured quotas.
#[derive(Clone)]
pub struct StorageQuotaMatcher<'a> {
    env: &'a Env,
    contract: &'a Address,
    instance_quota: TierQuota,
    persistent_quota: TierQuota,
    temporary_quota: TierQuota,
}

impl<'a> fmt::Debug for StorageQuotaMatcher<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageQuotaMatcher")
            .field("contract", &self.contract)
            .field("instance_quota", &self.instance_quota)
            .field("persistent_quota", &self.persistent_quota)
            .field("temporary_quota", &self.temporary_quota)
            .finish()
    }
}

impl<'a> StorageQuotaMatcher<'a> {
    /// Create a new quota matcher for `contract` in `env`.
    pub fn new(env: &'a Env, contract: &'a Address) -> Self {
        Self {
            env,
            contract,
            instance_quota: TierQuota::Unconstrained,
            persistent_quota: TierQuota::Unconstrained,
            temporary_quota: TierQuota::Unconstrained,
        }
    }

    /// Set an exact count requirement for instance storage.
    pub fn exact_instance(mut self, count: usize) -> Self {
        self.instance_quota = TierQuota::Exact(count);
        self
    }

    /// Set an upper-bound quota for instance storage.
    pub fn max_instance(mut self, limit: usize) -> Self {
        self.instance_quota = TierQuota::Max(limit);
        self
    }

    /// Set a lower-bound quota for instance storage.
    pub fn min_instance(mut self, limit: usize) -> Self {
        self.instance_quota = TierQuota::Min(limit);
        self
    }

    /// Set an exact count requirement for persistent storage.
    pub fn exact_persistent(mut self, count: usize) -> Self {
        self.persistent_quota = TierQuota::Exact(count);
        self
    }

    /// Set an upper-bound quota for persistent storage.
    pub fn max_persistent(mut self, limit: usize) -> Self {
        self.persistent_quota = TierQuota::Max(limit);
        self
    }

    /// Set a lower-bound quota for persistent storage.
    pub fn min_persistent(mut self, limit: usize) -> Self {
        self.persistent_quota = TierQuota::Min(limit);
        self
    }

    /// Set an exact count requirement for temporary storage.
    pub fn exact_temporary(mut self, count: usize) -> Self {
        self.temporary_quota = TierQuota::Exact(count);
        self
    }

    /// Set an upper-bound quota for temporary storage.
    pub fn max_temporary(mut self, limit: usize) -> Self {
        self.temporary_quota = TierQuota::Max(limit);
        self
    }

    /// Set a lower-bound quota for temporary storage.
    pub fn min_temporary(mut self, limit: usize) -> Self {
        self.temporary_quota = TierQuota::Min(limit);
        self
    }

    /// Set quota directly for a specific tier.
    pub fn quota(mut self, tier: StorageTier, quota: TierQuota) -> Self {
        match tier {
            StorageTier::Instance => self.instance_quota = quota,
            StorageTier::Persistent => self.persistent_quota = quota,
            StorageTier::Temporary => self.temporary_quota = quota,
        }
        self
    }

    /// Execute the inspection and return detailed results per tier.
    pub fn inspect(&self) -> BTreeMap<StorageTier, TierInspection> {
        let snapshot = StorageSnapshot::capture(self.env, self.contract);

        let tiers = [
            (StorageTier::Instance, self.instance_quota),
            (StorageTier::Persistent, self.persistent_quota),
            (StorageTier::Temporary, self.temporary_quota),
        ];

        let mut results = BTreeMap::new();
        for (tier, quota) in tiers {
            let entries = snapshot.in_tier(tier);
            let count = entries.len();
            let keys: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
            let passed = quota.is_satisfied(count);

            results.insert(
                tier,
                TierInspection {
                    tier,
                    quota,
                    count,
                    keys,
                    passed,
                },
            );
        }

        results
    }

    /// Assert that all configured storage tier quotas are met, panicking with a
    /// formatted breakdown table upon any breach.
    pub fn assert_matches(&self) {
        let inspections = self.inspect();
        let breaches: Vec<&TierInspection> =
            inspections.values().filter(|insp| !insp.passed).collect();

        if !breaches.is_empty() {
            let table = format_quota_table(&inspections);
            let mut summary = Vec::new();
            for b in &breaches {
                summary.push(format!(
                    "{} (found {}, expected {})",
                    b.tier.as_str(),
                    b.count,
                    b.quota.description()
                ));
            }

            panic!(
                "\nStorage allocation quota exceeded for contract:\nBreached tiers: {}\n\n{}",
                summary.join(", "),
                table
            );
        }
    }
}

/// Format a clean breakdown table showing tier counts, quotas, status, and keys.
fn format_quota_table(inspections: &BTreeMap<StorageTier, TierInspection>) -> String {
    let mut out = String::new();
    out.push_str("| Tier        | Count | Quota          | Status | Keys                                       |\n");
    out.push_str("|-------------|-------|----------------|--------|--------------------------------------------|\n");

    for tier in &[
        StorageTier::Instance,
        StorageTier::Persistent,
        StorageTier::Temporary,
    ] {
        if let Some(insp) = inspections.get(tier) {
            let status = if insp.passed { "PASS" } else { "BREACH" };

            let keys_str = if insp.keys.is_empty() {
                "(none)".to_string()
            } else if insp.keys.len() <= 4 {
                insp.keys
                    .iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                let first_few = insp.keys[..3]
                    .iter()
                    .map(|k| format!("\"{k}\""))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{first_few}, ... (total {})", insp.keys.len())
            };

            out.push_str(&format!(
                "| {:<11} | {:<5} | {:<14} | {:<6} | {:<42} |\n",
                insp.tier.as_str(),
                insp.count,
                insp.quota.description(),
                status,
                keys_str
            ));
        }
    }

    out
}

/// Helper macro to apply a tier quota field to a [`StorageQuotaMatcher`].
#[doc(hidden)]
#[macro_export]
macro_rules! __apply_quota {
    ($matcher:expr, instance, $val:expr) => {
        $matcher.exact_instance($val)
    };
    ($matcher:expr, persistent, $val:expr) => {
        $matcher.exact_persistent($val)
    };
    ($matcher:expr, temporary, $val:expr) => {
        $matcher.exact_temporary($val)
    };
    ($matcher:expr, max_instance, $val:expr) => {
        $matcher.max_instance($val)
    };
    ($matcher:expr, max_persistent, $val:expr) => {
        $matcher.max_persistent($val)
    };
    ($matcher:expr, max_temporary, $val:expr) => {
        $matcher.max_temporary($val)
    };
    ($matcher:expr, min_instance, $val:expr) => {
        $matcher.min_instance($val)
    };
    ($matcher:expr, min_persistent, $val:expr) => {
        $matcher.min_persistent($val)
    };
    ($matcher:expr, min_temporary, $val:expr) => {
        $matcher.min_temporary($val)
    };
}

/// Assert storage tier entry count quotas for instance, persistent, and temporary tiers.
///
/// Supports exact counts (`instance: 1`), upper bounds (`max_instance: 5`), and lower
/// bounds (`min_instance: 1`).
///
/// # Examples
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_storage_tier_count;
///
/// // Enforce exact count and upper bounds using block syntax:
/// assert_storage_tier_count!(env, contract, {
///     instance: 1,
///     max_persistent: 10,
///     temporary: 0,
/// });
///
/// // Alternatively, specify as comma-separated assignments:
/// assert_storage_tier_count!(env, contract, max_instance = 3, persistent = 5);
/// ```
#[macro_export]
macro_rules! assert_storage_tier_count {
    ($env:expr, $contract:expr, { $($field:ident : $val:expr),* $(,)? }) => {{
        let mut matcher = $crate::storage::StorageQuotaMatcher::new(&$env, &$contract);
        $(
            matcher = $crate::__apply_quota!(matcher, $field, $val);
        )*
        matcher.assert_matches();
    }};
    ($env:expr, $contract:expr, $($field:ident = $val:expr),* $(,)?) => {{
        let mut matcher = $crate::storage::StorageQuotaMatcher::new(&$env, &$contract);
        $(
            matcher = $crate::__apply_quota!(matcher, $field, $val);
        )*
        matcher.assert_matches();
    }};
}

/// Assert that `key` exists in none of the contract's storage tiers.
///
/// The key is the rendered form a [`StorageSnapshot`] reports — a symbol's
/// text, a number, `[a, b]` for a vector, `{k: v}` for a map — the same form
/// [`assert_storage_tier`](soroban_testkit_core::assert_storage_tier) takes.
///
/// # Panics
///
/// Panics if any tier still holds the key, showing every surviving entry.
pub fn assert_storage_key_deleted(env: &Env, contract: &Address, key: &str) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors: Vec<&StorageEntry> = snapshot
        .entries()
        .iter()
        .filter(|entry| entry.key == key)
        .collect();

    assert!(
        survivors.is_empty(),
        "Expected storage key {:?} to be deleted from all tiers, but it still exists:\n{}",
        key,
        describe(&survivors),
    );
}

/// Assert that `key` no longer exists in one storage tier.
///
/// The key may still live in another tier; only `tier` is inspected.
///
/// # Panics
///
/// Panics if `tier` still holds the key, showing the surviving entry.
pub fn assert_storage_key_deleted_in(env: &Env, contract: &Address, key: &str, tier: StorageTier) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors: Vec<&StorageEntry> = snapshot
        .in_tier(tier)
        .into_iter()
        .filter(|entry| entry.key == key)
        .collect();

    assert!(
        survivors.is_empty(),
        "Expected storage key {:?} to be deleted from the {} tier, but it still exists:\n{}",
        key,
        tier.as_str(),
        describe(&survivors),
    );
}

/// Assert that the contract holds no live storage entry in any tier.
///
/// A contract that never wrote storage passes, as does one whose keys were
/// all deleted. Entries rewritten to zero, `None` or another empty value do
/// not pass: they are still stored.
///
/// # Panics
///
/// Panics if any tier holds an entry, listing every survivor with its bytes.
pub fn assert_storage_empty(env: &Env, contract: &Address) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors: Vec<&StorageEntry> = snapshot.entries().iter().collect();

    assert!(
        survivors.is_empty(),
        "Expected contract storage to be empty, but {} entr{} survived:\n{}",
        survivors.len(),
        if survivors.len() == 1 { "y" } else { "ies" },
        describe(&survivors),
    );
}

/// Assert that one storage tier of the contract holds no live entry.
///
/// The other tiers are not inspected and may hold entries.
///
/// # Panics
///
/// Panics if `tier` holds an entry, showing it with its value and bytes.
pub fn assert_storage_empty_in(env: &Env, contract: &Address, tier: StorageTier) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors = snapshot.in_tier(tier);

    assert!(
        survivors.is_empty(),
        "Expected the {} storage tier to be empty, but {} entr{} survived:\n{}",
        tier.as_str(),
        survivors.len(),
        if survivors.len() == 1 { "y" } else { "ies" },
        describe(&survivors),
    );
}

/// Asserts that a storage key no longer exists in any tier.
///
/// Pass a [`StorageTier`] as a fourth argument to inspect one tier only.
///
/// # Example
///
/// ```rust
/// use soroban_testkit_assert::{assert_storage_key_deleted, storage::StorageTier};
/// # use soroban_sdk::{Address, Env};
/// # fn example(env: &Env, contract: &Address) {
/// assert_storage_key_deleted!(env, contract, "balance");
/// assert_storage_key_deleted!(env, contract, "balance", StorageTier::Persistent);
/// # }
/// ```
#[macro_export]
macro_rules! assert_storage_key_deleted {
    ($env:expr, $contract:expr, $key:expr $(,)?) => {
        $crate::storage::assert_storage_key_deleted($env, $contract, $key)
    };
    ($env:expr, $contract:expr, $key:expr, $tier:expr $(,)?) => {
        $crate::storage::assert_storage_key_deleted_in($env, $contract, $key, $tier)
    };
}

/// Asserts that a contract's storage, or one tier of it, is empty.
///
/// Pass a [`StorageTier`] as a third argument to inspect one tier only.
///
/// # Example
///
/// ```rust
/// use soroban_testkit_assert::{assert_storage_empty, storage::StorageTier};
/// # use soroban_sdk::{Address, Env};
/// # fn example(env: &Env, contract: &Address) {
/// assert_storage_empty!(env, contract);
/// assert_storage_empty!(env, contract, StorageTier::Temporary);
/// # }
/// ```
#[macro_export]
macro_rules! assert_storage_empty {
    ($env:expr, $contract:expr $(,)?) => {
        $crate::storage::assert_storage_empty($env, $contract)
    };
    ($env:expr, $contract:expr, $tier:expr $(,)?) => {
        $crate::storage::assert_storage_empty_in($env, $contract, $tier)
    };
}

/// How much of a surviving value's XDR to quote in a failure message.
const MAX_QUOTED_BYTES: usize = 64;

fn describe(entries: &[&StorageEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("  {}", describe_entry(entry)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn describe_entry(entry: &StorageEntry) -> String {
    let bytes = entry.value_bytes();
    format!(
        "{} = {} [{} byte{}: {}]",
        entry.qualified(),
        entry.value,
        bytes.len(),
        if bytes.len() == 1 { "" } else { "s" },
        hex(bytes),
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(MAX_QUOTED_BYTES * 2 + 1);
    for byte in bytes.iter().take(MAX_QUOTED_BYTES) {
        rendered.push_str(&format!("{byte:02x}"));
    }
    if bytes.len() > MAX_QUOTED_BYTES {
        rendered.push('…');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env,
    };

    #[contract]
    pub struct QuotaTestContract;

    #[contractimpl]
    impl QuotaTestContract {
        pub fn init(env: Env) {
            env.storage().instance().set(&symbol_short!("cfg"), &1u32);

            env.storage()
                .persistent()
                .set(&symbol_short!("p_1"), &10u64);
            env.storage()
                .persistent()
                .set(&symbol_short!("p_2"), &20u64);
            env.storage()
                .persistent()
                .set(&symbol_short!("p_3"), &30u64);

            env.storage().temporary().set(&symbol_short!("t_1"), &1u32);
        }
    }

    #[test]
    fn test_exact_counts_pass() {
        let env = Env::default();
        let contract_id = env.register(QuotaTestContract, ());
        let client = QuotaTestContractClient::new(&env, &contract_id);
        client.init();

        assert_storage_tier_count!(env, contract_id, {
            instance: 1,
            persistent: 3,
            temporary: 1,
        });

        // Also test equals syntax
        assert_storage_tier_count!(
            env,
            contract_id,
            instance = 1,
            persistent = 3,
            temporary = 1
        );
    }

    #[test]
    fn test_max_quotas_pass() {
        let env = Env::default();
        let contract_id = env.register(QuotaTestContract, ());
        let client = QuotaTestContractClient::new(&env, &contract_id);
        client.init();

        assert_storage_tier_count!(env, contract_id, {
            max_instance: 5,
            max_persistent: 10,
            max_temporary: 2,
        });

        assert_storage_tier_count!(env, contract_id, max_instance = 1, max_persistent = 3);
    }

    #[test]
    fn test_empty_contract_quotas() {
        let env = Env::default();
        let empty_addr = Address::generate(&env);

        assert_storage_tier_count!(env, empty_addr, {
            instance: 0,
            persistent: 0,
            temporary: 0,
        });

        assert_storage_tier_count!(env, empty_addr, max_instance = 0, max_persistent = 0);
    }

    #[test]
    #[should_panic(expected = "Storage allocation quota exceeded")]
    fn test_exact_count_breach_panics() {
        let env = Env::default();
        let contract_id = env.register(QuotaTestContract, ());
        let client = QuotaTestContractClient::new(&env, &contract_id);
        client.init();

        // Persistent count is 3, expecting 2
        assert_storage_tier_count!(env, contract_id, {
            instance: 1,
            persistent: 2,
        });
    }

    #[test]
    #[should_panic(expected = "Storage allocation quota exceeded")]
    fn test_max_quota_breach_panics() {
        let env = Env::default();
        let contract_id = env.register(QuotaTestContract, ());
        let client = QuotaTestContractClient::new(&env, &contract_id);
        client.init();

        // Persistent count is 3, max is 2
        assert_storage_tier_count!(env, contract_id, max_persistent = 2);
    }

    #[test]
    fn test_table_formatting_contains_keys() {
        let env = Env::default();
        let contract_id = env.register(QuotaTestContract, ());
        let client = QuotaTestContractClient::new(&env, &contract_id);
        client.init();

        let matcher = StorageQuotaMatcher::new(&env, &contract_id)
            .exact_instance(1)
            .max_persistent(5)
            .exact_temporary(1);

        let inspections = matcher.inspect();
        let table = format_quota_table(&inspections);

        assert!(table.contains("| instance"));
        assert!(table.contains("| persistent"));
        assert!(table.contains("| temporary"));
        assert!(table.contains("PASS"));
        assert!(table.contains("\"cfg\""));
        assert!(table.contains("\"p_1\""));
    }
}

#[cfg(test)]
mod deletion_tests {
    use super::StorageTier;
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env, Symbol,
    };

    #[contract]
    pub struct Store;

    #[contractimpl]
    impl Store {
        pub fn seed(env: Env) {
            env.storage().persistent().set(&count_key(&env), &42u32);
            env.storage().temporary().set(&ttl_key(&env), &7u32);
            env.storage().instance().set(&admin_key(), &1u32);
        }

        pub fn erase_count(env: Env) {
            env.storage().persistent().remove(&count_key(&env));
        }

        pub fn store_zero(env: Env) {
            env.storage().persistent().set(&count_key(&env), &0u32);
        }

        pub fn store_none(env: Env) {
            env.storage()
                .persistent()
                .set(&count_key(&env), &Option::<u32>::None);
        }

        pub fn store_empty_map(env: Env) {
            env.storage().persistent().set(
                &count_key(&env),
                &soroban_sdk::Map::<Symbol, u32>::new(&env),
            );
        }

        pub fn clear(env: Env) {
            env.storage().persistent().remove(&count_key(&env));
            env.storage().temporary().remove(&ttl_key(&env));
            env.storage().instance().remove(&admin_key());
        }
    }

    fn count_key(env: &Env) -> Symbol {
        Symbol::new(env, "count")
    }

    fn ttl_key(env: &Env) -> Symbol {
        Symbol::new(env, "ttl")
    }

    fn admin_key() -> Symbol {
        symbol_short!("admin")
    }

    fn registered() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(Store, ());
        (env, contract)
    }

    fn seeded() -> (Env, Address) {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).seed();
        (env, contract)
    }

    #[test]
    fn deleted_key_is_absent_from_every_tier() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).erase_count();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    fn deleted_key_in_a_tier_passes_while_other_tiers_hold_keys() {
        let (env, contract) = seeded();
        let client = StoreClient::new(&env, &contract);
        client.erase_count();

        assert_storage_key_deleted!(&env, &contract, "count", StorageTier::Persistent);

        // "ttl" never existed in the persistent tier, so naming that tier passes
        // even though the key is alive in temporary storage.
        assert_storage_key_deleted!(&env, &contract, "ttl", StorageTier::Persistent);
    }

    #[test]
    #[should_panic(expected = "persistent:count = 42")]
    fn existing_key_fails_across_all_tiers() {
        let (env, contract) = seeded();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    #[should_panic(expected = "to be deleted from the persistent tier")]
    fn existing_key_fails_in_the_named_tier() {
        let (env, contract) = seeded();

        assert_storage_key_deleted!(&env, &contract, "count", StorageTier::Persistent);
    }

    #[test]
    #[should_panic(expected = "persistent:count = 0")]
    fn a_key_rewritten_to_zero_is_not_deleted() {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).store_zero();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    #[should_panic(expected = "persistent:count = void")]
    fn a_key_holding_option_none_is_not_deleted() {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).store_none();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    #[should_panic(expected = "persistent:count = {}")]
    fn a_key_holding_an_empty_value_is_not_deleted() {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).store_empty_map();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    fn an_untouched_contract_has_empty_storage() {
        let (env, contract) = registered();

        assert_storage_empty!(&env, &contract);
        assert_storage_empty!(&env, &contract, StorageTier::Instance);
        assert_storage_empty!(&env, &contract, StorageTier::Persistent);
        assert_storage_empty!(&env, &contract, StorageTier::Temporary);
    }

    #[test]
    fn a_cleared_contract_has_empty_storage() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).clear();

        assert_storage_empty!(&env, &contract);
    }

    #[test]
    fn an_empty_tier_passes_while_other_tiers_hold_keys() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).erase_count();

        assert_storage_empty!(&env, &contract, StorageTier::Persistent);

        // The other tiers are untouched and still hold their keys.
        assert_storage_key_deleted!(&env, &contract, "ttl", StorageTier::Persistent);
        assert_storage_key_deleted!(&env, &contract, "admin", StorageTier::Persistent);
    }

    #[test]
    #[should_panic(expected = "3 entries survived")]
    fn a_non_empty_contract_lists_every_survivor() {
        let (env, contract) = seeded();

        assert_storage_empty!(&env, &contract);
    }

    #[test]
    #[should_panic(expected = "temporary:ttl = 7 [8 bytes:")]
    fn survivors_are_reported_with_their_bytes() {
        let (env, contract) = seeded();

        assert_storage_empty!(&env, &contract);
    }

    #[test]
    #[should_panic(expected = "temporary storage tier to be empty")]
    fn a_non_empty_tier_fails() {
        let (env, contract) = seeded();

        assert_storage_empty!(&env, &contract, StorageTier::Temporary);
    }

    #[test]
    fn a_non_contract_address_captures_as_empty() {
        let env = Env::default();
        let account = Address::generate(&env);

        assert_storage_empty!(&env, &account);
        assert_storage_key_deleted!(&env, &account, "count");
    }
}
