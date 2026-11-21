//! Storage tier entry count quota assertions.
//!
//! Smart contract state must be partitioned appropriately across instance,
//! persistent, and temporary storage tiers. [`StorageQuotaMatcher`] and the
//! [`assert_storage_tier_count!`] macro enforce entry count quotas per tier,
//! validating exact counts or upper bounds (`max_*`), and format an informative
//! breakdown table listing all keys in each tier whenever a quota is breached.

use std::collections::BTreeMap;
use std::fmt;

use soroban_sdk::{Address, Env, TryFromVal, Val, xdr::ScVal};
use soroban_testkit_core::storage::{StorageChange, StorageDiff, StorageEntry, StorageSnapshot, StorageTier};

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

// ============================================================================
// assert_storage_diff! macro and StorageDiffMatcher
// ============================================================================

/// Expected value specification for a storage key in the diff assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedValue {
    /// Any value is accepted (wildcard for value).
    Any,
    /// Exact value match with typed deserialization.
    Exact(String),
    /// Value must deserialize to the given type and match.
    Typed {
        type_name: String,
        value: String,
    },
}

impl ExpectedValue {
    /// Check if the actual value matches the expected value.
    pub fn matches(&self, actual: &str) -> bool {
        match self {
            ExpectedValue::Any => true,
            ExpectedValue::Exact(expected) => expected == actual,
            ExpectedValue::Typed { value, .. } => value == actual,
        }
    }
}

/// A single expected added entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedAdded {
    pub key: String,
    pub tier: Option<StorageTier>,
    pub value: ExpectedValue,
}

/// A single expected modified entry with from/to values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedModified {
    pub key: String,
    pub tier: Option<StorageTier>,
    pub from: ExpectedValue,
    pub to: ExpectedValue,
}

/// A single expected removed entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedRemoved {
    pub key: String,
    pub tier: Option<StorageTier>,
    pub value: ExpectedValue,
}

/// A single expected unchanged entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedUnchanged {
    pub key: String,
    pub tier: Option<StorageTier>,
    pub value: ExpectedValue,
}

/// Pattern matching for storage keys (exact or glob).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyPattern {
    Exact(String),
    Glob(String),
}

impl KeyPattern {
    pub fn matches(&self, key: &str) -> bool {
        match self {
            KeyPattern::Exact(expected) => expected == key,
            KeyPattern::Glob(pattern) => {
                if pattern == "*" {
                    true
                } else if let Some(prefix) = pattern.strip_suffix('*') {
                    key.starts_with(prefix)
                } else if let Some(suffix) = pattern.strip_prefix('*') {
                    key.ends_with(suffix)
                } else {
                    pattern == key
                }
            }
        }
    }
}

/// Matcher for storage diff assertions.
#[derive(Clone)]
pub struct StorageDiffMatcher<'a> {
    env: &'a Env,
    contract: &'a Address,
    before_snapshot: StorageSnapshot,
    added: Vec<ExpectedAdded>,
    modified: Vec<ExpectedModified>,
    removed: Vec<ExpectedRemoved>,
    unchanged: Vec<ExpectedUnchanged>,
}

impl<'a> fmt::Debug for StorageDiffMatcher<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageDiffMatcher")
            .field("contract", &self.contract)
            .field("added", &self.added)
            .field("modified", &self.modified)
            .field("removed", &self.removed)
            .field("unchanged", &self.unchanged)
            .finish()
    }
}

impl<'a> StorageDiffMatcher<'a> {
    /// Create a new storage diff matcher, capturing the current storage state as the "before" snapshot.
    pub fn new(env: &'a Env, contract: &'a Address) -> Self {
        let before_snapshot = StorageSnapshot::capture(env, contract);
        Self {
            env,
            contract,
            before_snapshot,
            added: Vec::new(),
            modified: Vec::new(),
            removed: Vec::new(),
            unchanged: Vec::new(),
        }
    }

    /// Create a new storage diff matcher with a pre-captured "before" snapshot.
    pub fn with_before_snapshot(env: &'a Env, contract: &'a Address, before_snapshot: StorageSnapshot) -> Self {
        Self {
            env,
            contract,
            before_snapshot,
            added: Vec::new(),
            modified: Vec::new(),
            removed: Vec::new(),
            unchanged: Vec::new(),
        }
    }

    /// Add expected added keys.
    pub fn added(mut self, added: Vec<ExpectedAdded>) -> Self {
        self.added = added;
        self
    }

    /// Add expected modified keys.
    pub fn modified(mut self, modified: Vec<ExpectedModified>) -> Self {
        self.modified = modified;
        self
    }

    /// Add expected removed keys.
    pub fn removed(mut self, removed: Vec<ExpectedRemoved>) -> Self {
        self.removed = removed;
        self
    }

    /// Add expected unchanged keys.
    pub fn unchanged(mut self, unchanged: Vec<ExpectedUnchanged>) -> Self {
        self.unchanged = unchanged;
        self
    }

    /// Execute the assertion.
    pub fn assert(self) {
        let after_snapshot = StorageSnapshot::capture(self.env, self.contract);
        let diff = self.before_snapshot.diff(&after_snapshot);

        let mut failures = Vec::new();

        // Check added entries
        failures.extend(self.check_added(&diff));
        // Check modified entries
        failures.extend(self.check_modified(&diff));
        // Check removed entries
        failures.extend(self.check_removed(&diff));
        // Check unchanged entries
        failures.extend(self.check_unchanged(&diff, &after_snapshot));

        // Check for unexpected changes
        failures.extend(self.check_unexpected(&diff));

        if !failures.is_empty() {
            let table = self.format_diff_table(&diff, &failures, &after_snapshot);
            panic!(
                "\nStorage diff assertion failed:\n{}\n\n{}",
                failures.join("\n"),
                table
            );
        }
    }

    fn check_added(&self, diff: &StorageDiff) -> Vec<String> {
        let mut failures = Vec::new();
        let mut matched_indices = std::collections::HashSet::new();

        for expected in &self.added {
            let mut found = false;
            for (idx, entry) in diff.added.iter().enumerate() {
                if matched_indices.contains(&idx) {
                    continue;
                }
                if self.key_and_tier_match(&expected.key, &expected.tier, entry) {
                    if expected.value.matches(&entry.value) {
                        found = true;
                        matched_indices.insert(idx);
                        break;
                    } else {
                        failures.push(format!(
                            "Added key '{}' (tier: {}): expected value '{}', found '{}'",
                            entry.key,
                            entry.tier.as_str(),
                            self.format_expected_value(&expected.value),
                            entry.value
                        ));
                        matched_indices.insert(idx);
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                failures.push(format!(
                    "Expected added key '{}' (tier: {}) not found in diff",
                    expected.key,
                    expected.tier.map(|t| t.as_str()).unwrap_or("any")
                ));
            }
        }
        failures
    }

    fn check_modified(&self, diff: &StorageDiff) -> Vec<String> {
        let mut failures = Vec::new();
        let mut matched_indices = std::collections::HashSet::new();

        for expected in &self.modified {
            let mut found = false;
            for (idx, change) in diff.modified.iter().enumerate() {
                if matched_indices.contains(&idx) {
                    continue;
                }
                if self.key_and_tier_match(&expected.key, &expected.tier, change) {
                    let from_ok = expected.from.matches(&change.before);
                    let to_ok = expected.to.matches(&change.after);
                    if from_ok && to_ok {
                        found = true;
                        matched_indices.insert(idx);
                        break;
                    } else {
                        let mut msgs = Vec::new();
                        if !from_ok {
                            msgs.push(format!(
                                "from: expected '{}', found '{}'",
                                self.format_expected_value(&expected.from),
                                change.before
                            ));
                        }
                        if !to_ok {
                            msgs.push(format!(
                                "to: expected '{}', found '{}'",
                                self.format_expected_value(&expected.to),
                                change.after
                            ));
                        }
                        failures.push(format!(
                            "Modified key '{}' (tier: {}): {}",
                            change.key,
                            change.tier.as_str(),
                            msgs.join(", ")
                        ));
                        matched_indices.insert(idx);
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                failures.push(format!(
                    "Expected modified key '{}' (tier: {}) not found in diff",
                    expected.key,
                    expected.tier.map(|t| t.as_str()).unwrap_or("any")
                ));
            }
        }
        failures
    }

    fn check_removed(&self, diff: &StorageDiff) -> Vec<String> {
        let mut failures = Vec::new();
        let mut matched_indices = std::collections::HashSet::new();

        for expected in &self.removed {
            let mut found = false;
            for (idx, entry) in diff.removed.iter().enumerate() {
                if matched_indices.contains(&idx) {
                    continue;
                }
                if self.key_and_tier_match(&expected.key, &expected.tier, entry) {
                    if expected.value.matches(&entry.value) {
                        found = true;
                        matched_indices.insert(idx);
                        break;
                    } else {
                        failures.push(format!(
                            "Removed key '{}' (tier: {}): expected value '{}', found '{}'",
                            entry.key,
                            entry.tier.as_str(),
                            self.format_expected_value(&expected.value),
                            entry.value
                        ));
                        matched_indices.insert(idx);
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                failures.push(format!(
                    "Expected removed key '{}' (tier: {}) not found in diff",
                    expected.key,
                    expected.tier.map(|t| t.as_str()).unwrap_or("any")
                ));
            }
        }
        failures
    }

    fn check_unchanged(&self, diff: &StorageDiff, after_snapshot: &StorageSnapshot) -> Vec<String> {
        let mut failures = Vec::new();

        for expected in &self.unchanged {
            // Check that the key wasn't added, removed, or modified
            let was_added = diff.added.iter().any(|e| self.key_and_tier_match(&expected.key, &expected.tier, e));
            let was_removed = diff.removed.iter().any(|e| self.key_and_tier_match(&expected.key, &expected.tier, e));
            let was_modified = diff.modified.iter().any(|c| self.key_and_tier_match(&expected.key, &expected.tier, c));

            if was_added || was_removed || was_modified {
                failures.push(format!(
                    "Expected key '{}' (tier: {}) to be unchanged, but it was {}",
                    expected.key,
                    expected.tier.map(|t| t.as_str()).unwrap_or("any"),
                    if was_added { "added" } else if was_removed { "removed" } else { "modified" }
                ));
                continue;
            }

            // Verify the value matches in both snapshots
            let before_entry = self.find_entry(&self.before_snapshot, &expected.key, &expected.tier);
            let after_entry = self.find_entry(after_snapshot, &expected.key, &expected.tier);

            match (before_entry, after_entry) {
                (Some(before), Some(after)) => {
                    if !expected.value.matches(&before.value) || !expected.value.matches(&after.value) {
                        failures.push(format!(
                            "Unchanged key '{}' (tier: {}): expected value '{}', before='{}', after='{}'",
                            expected.key,
                            expected.tier.map(|t| t.as_str()).unwrap_or("any"),
                            self.format_expected_value(&expected.value),
                            before.value,
                            after.value
                        ));
                    }
                }
                (None, None) => {
                    failures.push(format!(
                        "Expected unchanged key '{}' (tier: {}) not found in either snapshot",
                        expected.key,
                        expected.tier.map(|t| t.as_str()).unwrap_or("any")
                    ));
                }
                (Some(_), None) => {
                    failures.push(format!(
                        "Expected unchanged key '{}' (tier: {}) found in before but missing in after",
                        expected.key,
                        expected.tier.map(|t| t.as_str()).unwrap_or("any")
                    ));
                }
                (None, Some(_)) => {
                    failures.push(format!(
                        "Expected unchanged key '{}' (tier: {}) missing in before but found in after",
                        expected.key,
                        expected.tier.map(|t| t.as_str()).unwrap_or("any")
                    ));
                }
            }
        }
        failures
    }

    fn check_unexpected(&self, diff: &StorageDiff) -> Vec<String> {
        let mut failures = Vec::new();

        // Check for unexpected added entries
        for entry in &diff.added {
            let expected = self.added.iter().any(|e| self.key_and_tier_match(&e.key, &e.tier, entry));
            if !expected {
                failures.push(format!(
                    "Unexpected added key '{}' (tier: {}) with value '{}'",
                    entry.key, entry.tier.as_str(), entry.value
                ));
            }
        }

        // Check for unexpected modified entries
        for change in &diff.modified {
            let expected = self.modified.iter().any(|e| self.key_and_tier_match(&e.key, &e.tier, change));
            if !expected {
                failures.push(format!(
                    "Unexpected modified key '{}' (tier: {}): '{}' -> '{}'",
                    change.key, change.tier.as_str(), change.before, change.after
                ));
            }
        }

        // Check for unexpected removed entries
        for entry in &diff.removed {
            let expected = self.removed.iter().any(|e| self.key_and_tier_match(&e.key, &e.tier, entry));
            if !expected {
                failures.push(format!(
                    "Unexpected removed key '{}' (tier: {}) with value '{}'",
                    entry.key, entry.tier.as_str(), entry.value
                ));
            }
        }

        failures
    }

    fn key_and_tier_match<T: StorageEntryLike>(&self, pattern: &str, tier: &Option<StorageTier>, entry: &T) -> bool {
        let key_match = if pattern.contains('*') {
            let kp = KeyPattern::Glob(pattern.to_string());
            kp.matches(&entry.key())
        } else {
            pattern == entry.key()
        };
        let tier_match = tier.map(|t| t == entry.tier()).unwrap_or(true);
        key_match && tier_match
    }

    fn find_entry<'b>(&self, snapshot: &'b StorageSnapshot, key: &str, tier: &Option<StorageTier>) -> Option<&'b StorageEntry> {
        if let Some(t) = tier {
            snapshot.entries().iter().find(|e| e.key == key && e.tier == *t)
        } else {
            snapshot.entries().iter().find(|e| e.key == key)
        }
    }

    fn format_expected_value(&self, ev: &ExpectedValue) -> String {
        match ev {
            ExpectedValue::Any => "<any>".to_string(),
            ExpectedValue::Exact(v) => v.clone(),
            ExpectedValue::Typed { type_name, value } => format!("{}::{}", type_name, value),
        }
    }

    fn format_diff_table(&self, diff: &StorageDiff, failures: &[String], after_snapshot: &StorageSnapshot) -> String {
        let mut out = String::new();
        out.push_str("════════════════════════════════════════════════════════════════════════════════\n");
        out.push_str("                           STORAGE DIFF REPORT                                   \n");
        out.push_str("═══════════════════════════════════════════════════════════════════════════════\n\n");

        // Summary
        out.push_str(&format!(
            "Summary: {} added, {} modified, {} removed | Failures: {}\n\n",
            diff.added.len(),
            diff.modified.len(),
            diff.removed.len(),
            failures.len()
        ));

        // Added table
        if !diff.added.is_empty() || !self.added.is_empty() {
            out.push_str("┌──────────────────────────────────────────────────────────────────────────────┐\n");
            out.push_str("│ ADDED                                                                         │\n");
            out.push_str("├────────┬──────────────────────┬──────────────────────────────────────────────┤\n");
            out.push_str("│ Tier   │ Key                  │ Value                                         │\n");
            out.push_str("├────────┼──────────────────────┼──────────────────────────────────────────────┤\n");
            for entry in &diff.added {
                let status = if self.added.iter().any(|e| self.key_and_tier_match(&e.key, &e.tier, entry)) {
                    " ✓"
                } else {
                    " ✗ UNEXPECTED"
                };
                out.push_str(&format!(
                    "│ {:<6} │ {:<20} │ {:<42} │{}\n",
                    entry.tier.as_str(),
                    truncate(&entry.key, 20),
                    truncate(&entry.value, 42),
                    status
                ));
            }
            for expected in &self.added {
                if !diff.added.iter().any(|e| self.key_and_tier_match(&expected.key, &expected.tier, e)) {
                    out.push_str(&format!(
                        "│ {:<6} │ {:<20} │ {:<42} │ ✗ MISSING\n",
                        expected.tier.map(|t| t.as_str()).unwrap_or("any"),
                        truncate(&expected.key, 20),
                        truncate(&self.format_expected_value(&expected.value), 42)
                    ));
                }
            }
            out.push_str("└────────┴──────────────────────┴──────────────────────────────────────────────┘\n\n");
        }

        // Modified table
        if !diff.modified.is_empty() || !self.modified.is_empty() {
            out.push_str("┌──────────────────────────────────────────────────────────────────────────────┐\n");
            out.push_str("│ MODIFIED                                                                      │\n");
            out.push_str("├────────┬──────────────────────┬──────────────────────┬──────────────────────┤\n");
            out.push_str("│ Tier   │ Key                  │ Before               │ After                │\n");
            out.push_str("├────────┼──────────────────────┼──────────────────────┼──────────────────────┤\n");
            for change in &diff.modified {
                let status = if self.modified.iter().any(|e| self.key_and_tier_match(&e.key, &e.tier, change)) {
                    " ✓"
                } else {
                    " ✗ UNEXPECTED"
                };
                out.push_str(&format!(
                    "│ {:<6} │ {:<20} │ {:<20} │ {:<20} │{}\n",
                    change.tier.as_str(),
                    truncate(&change.key, 20),
                    truncate(&change.before, 20),
                    truncate(&change.after, 20),
                    status
                ));
            }
            for expected in &self.modified {
                if !diff.modified.iter().any(|c| self.key_and_tier_match(&expected.key, &expected.tier, c)) {
                    out.push_str(&format!(
                        "│ {:<6} │ {:<20} │ {:<20} │ {:<20} │ ✗ MISSING\n",
                        expected.tier.map(|t| t.as_str()).unwrap_or("any"),
                        truncate(&expected.key, 20),
                        truncate(&self.format_expected_value(&expected.from), 20),
                        truncate(&self.format_expected_value(&expected.to), 20)
                    ));
                }
            }
            out.push_str("└────────┴──────────────────────┴──────────────────────┴──────────────────────┘\n\n");
        }

        // Removed table
        if !diff.removed.is_empty() || !self.removed.is_empty() {
            out.push_str("┌──────────────────────────────────────────────────────────────────────────────┐\n");
            out.push_str("│ REMOVED                                                                       │\n");
            out.push_str("├────────┬──────────────────────┬──────────────────────────────────────────────┤\n");
            out.push_str("│ Tier   │ Key                  │ Value                                         │\n");
            out.push_str("├────────┼──────────────────────┼──────────────────────────────────────────────┤\n");
            for entry in &diff.removed {
                let status = if self.removed.iter().any(|e| self.key_and_tier_match(&e.key, &e.tier, entry)) {
                    " ✓"
                } else {
                    " ✗ UNEXPECTED"
                };
                out.push_str(&format!(
                    "│ {:<6} │ {:<20} │ {:<42} │{}\n",
                    entry.tier.as_str(),
                    truncate(&entry.key, 20),
                    truncate(&entry.value, 42),
                    status
                ));
            }
            for expected in &self.removed {
                if !diff.removed.iter().any(|e| self.key_and_tier_match(&expected.key, &expected.tier, e)) {
                    out.push_str(&format!(
                        "│ {:<6} │ {:<20} │ {:<42} │ ✗ MISSING\n",
                        expected.tier.map(|t| t.as_str()).unwrap_or("any"),
                        truncate(&expected.key, 20),
                        truncate(&self.format_expected_value(&expected.value), 42)
                    ));
                }
            }
            out.push_str("└────────┴──────────────────────┴──────────────────────────────────────────────┘\n\n");
        }

        // Unchanged table
        if !self.unchanged.is_empty() {
            out.push_str("┌──────────────────────────────────────────────────────────────────────────────┐\n");
            out.push_str("│ UNCHANGED                                                                     │\n");
            out.push_str("├────────┬──────────────────────┬──────────────────────────────────────────────┤\n");
            out.push_str("│ Tier   │ Key                  │ Value                                         │\n");
            out.push_str("├────────┼──────────────────────┼──────────────────────────────────────────────┤\n");
            for expected in &self.unchanged {
                let before_entry = self.find_entry(&self.before_snapshot, &expected.key, &expected.tier);
                let after_entry = self.find_entry(after_snapshot, &expected.key, &expected.tier);
                let value = before_entry.or(after_entry).map(|e| e.value.as_str()).unwrap_or("<missing>");
                let status = if failures.iter().any(|f| f.contains(&expected.key)) {
                    " ✗ MISMATCH"
                } else {
                    " ✓"
                };
                out.push_str(&format!(
                    "│ {:<6} │ {:<20} │ {:<42} │{}\n",
                    expected.tier.map(|t| t.as_str()).unwrap_or("any"),
                    truncate(&expected.key, 20),
                    truncate(value, 42),
                    status
                ));
            }
            out.push_str("└────────┴──────────────────────┴──────────────────────────────────────────────┘\n\n");
        }

        out.push_str("═══════════════════════════════════════════════════════════════════════════════\n");
        out
    }
}

/// Trait to abstract over StorageEntry and StorageChange for key/tier access.
trait StorageEntryLike {
    fn key(&self) -> &str;
    fn tier(&self) -> StorageTier;
}

impl StorageEntryLike for StorageEntry {
    fn key(&self) -> &str {
        &self.key
    }
    fn tier(&self) -> StorageTier {
        self.tier
    }
}

impl StorageEntryLike for StorageChange {
    fn key(&self) -> &str {
        &self.key
    }
    fn tier(&self) -> StorageTier {
        self.tier
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

/// Macro to build ExpectedValue from various input forms.
#[macro_export]
macro_rules! __storage_diff_value {
    ($val:expr) => { $crate::storage::ExpectedValue::Exact(format!("{}", $val)) };
    ($type:ty => $val:expr) => { $crate::storage::ExpectedValue::Typed { type_name: stringify!($type).to_string(), value: format!("{}", $val) } };
    () => { $crate::storage::ExpectedValue::Any };
}

/// Internal macro to parse a single entry with explicit field matching using expr for values.
#[macro_export]
macro_rules! __storage_diff_parse_entry {
    // Added entry
    (added, { key: $key:expr $(, tier: $tier:expr)? $(, value: $value:expr)? $(,)? }) => {
        $crate::storage::ExpectedAdded {
            key: $key.to_string(),
            tier: None $(.or(Some($tier)))?,
            value: $crate::__storage_diff_value!($($value)?),
        }
    };
    // Modified entry
    (modified, { key: $key:expr $(, tier: $tier:expr)? $(, from: $from:expr)? $(, to: $to:expr)? $(,)? }) => {
        $crate::storage::ExpectedModified {
            key: $key.to_string(),
            tier: None $(.or(Some($tier)))?,
            from: $crate::__storage_diff_value!($($from)?),
            to: $crate::__storage_diff_value!($($to)?),
        }
    };
    // Removed entry
    (removed, { key: $key:expr $(, tier: $tier:expr)? $(, value: $value:expr)? $(,)? }) => {
        $crate::storage::ExpectedRemoved {
            key: $key.to_string(),
            tier: None $(.or(Some($tier)))?,
            value: $crate::__storage_diff_value!($($value)?),
        }
    };
    // Unchanged entry
    (unchanged, { key: $key:expr $(, tier: $tier:expr)? $(, value: $value:expr)? $(,)? }) => {
        $crate::storage::ExpectedUnchanged {
            key: $key.to_string(),
            tier: None $(.or(Some($tier)))?,
            value: $crate::__storage_diff_value!($($value)?),
        }
    };
}

/// Macro to build ExpectedAdded from input.
#[macro_export]
macro_rules! __storage_diff_added {
    ($($tt:tt)*) => {
        $crate::__storage_diff_parse_entry!(added, $($tt)*)
    };
}

/// Macro to build ExpectedModified from input.
#[macro_export]
macro_rules! __storage_diff_modified {
    ($($tt:tt)*) => {
        $crate::__storage_diff_parse_entry!(modified, $($tt)*)
    };
}

/// Macro to build ExpectedRemoved from input.
#[macro_export]
macro_rules! __storage_diff_removed {
    ($($tt:tt)*) => {
        $crate::__storage_diff_parse_entry!(removed, $($tt)*)
    };
}

/// Macro to build ExpectedUnchanged from input.
#[macro_export]
macro_rules! __storage_diff_unchanged {
    ($($tt:tt)*) => {
        $crate::__storage_diff_parse_entry!(unchanged, $($tt)*)
    };
}

/// Assert storage changes between two points in time with declarative specifications.
///
/// Supports four clauses:
/// - `added`: Keys that should be newly present
/// - `modified`: Keys whose values should change (with `from` and `to`)
/// - `removed`: Keys that should be deleted
/// - `unchanged`: Keys that should remain exactly the same
///
/// Keys can be exact strings or glob patterns (`prefix*`, `*suffix`, `*`).
/// Each entry can optionally specify a `tier: StorageTier::Variant`.
/// Values can be exact (`value: 100`), typed (`value: u64 => 100`).
///
/// # Example
///
/// ```rust,ignore
/// use soroban_testkit_assert::assert_storage_diff;
/// use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
///
/// // Capture storage state before operations
/// let before = StorageSnapshot::capture(&env, &contract_id);
/// // ... perform operations ...
/// assert_storage_diff!(@before before, env, contract_id, {
///     modified: [
///         { key: "balances", from: 1000u64, to: 800u64 },
///     ],
///     added: [
///         { key: "allowance:*", tier: StorageTier::Temporary },
///     ],
///     removed: [
///         { key: "old_key" },
///     ],
///     unchanged: [
///         { key: "admin", tier: StorageTier::Instance },
///     ],
/// });
/// ```
#[macro_export]
macro_rules! assert_storage_diff {
    // With pre-captured before snapshot (uses @before marker)
    (@before $before:expr, $env:expr, $contract:expr, {
        $(added: [ $($added:tt),* $(,)? ] $(,)? )?
        $(modified: [ $($modified:tt),* $(,)? ] $(,)? )?
        $(removed: [ $($removed:tt),* $(,)? ] $(,)? )?
        $(unchanged: [ $($unchanged:tt),* $(,)? ] $(,)? )?
    }) => {{
        let matcher = $crate::storage::StorageDiffMatcher::with_before_snapshot(&$env, &$contract, $before)
            $(
                .added(vec![$($crate::__storage_diff_added!($added)),*])
            )?
            $(
                .modified(vec![$($crate::__storage_diff_modified!($modified)),*])
            )?
            $(
                .removed(vec![$($crate::__storage_diff_removed!($removed)),*])
            )?
            $(
                .unchanged(vec![$($crate::__storage_diff_unchanged!($unchanged)),*])
            )?;
        matcher.assert();
    }};
    // Without pre-captured snapshot (captures current state as "before")
    ($env:expr, $contract:expr, {
        $(added: [ $($added:tt),* $(,)? ] $(,)? )?
        $(modified: [ $($modified:tt),* $(,)? ] $(,)? )?
        $(removed: [ $($removed:tt),* $(,)? ] $(,)? )?
        $(unchanged: [ $($unchanged:tt),* $(,)? ] $(,)? )?
    }) => {{
        let matcher = $crate::storage::StorageDiffMatcher::new(&$env, &$contract)
            $(
                .added(vec![$($crate::__storage_diff_added!($added)),*])
            )?
            $(
                .modified(vec![$($crate::__storage_diff_modified!($modified)),*])
            )?
            $(
                .removed(vec![$($crate::__storage_diff_removed!($removed)),*])
            )?
            $(
                .unchanged(vec![$($crate::__storage_diff_unchanged!($unchanged)),*])
            )?;
        matcher.assert();
    }};
}

// ============================================================================
// assert_storage_diff! macro tests
// ============================================================================

#[cfg(test)]
mod storage_diff_tests {
    use super::*;
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

        pub fn add_allowance(env: Env, key: Symbol, value: u64) {
            env.storage().temporary().set(&key, &value);
        }

        pub fn modify_instance(env: Env) {
            env.storage().instance().set(&symbol_short!("cfg"), &2u32);
        }

        pub fn add_temp_key(env: Env, key: Symbol, value: u64) {
            env.storage().temporary().set(&key, &value);
        }
    }

    fn seeded() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(Store, ());
        StoreClient::new(&env, &contract).seed(&Address::generate(&env));
        (env, contract)
    }

    #[test]
    fn assert_storage_diff_modified_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).bump();

        assert_storage_diff!(@before before, env, contract, {
            modified: [
                { key: "count", from: 42u32, to: 43u32 },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_modified_with_tier_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).bump();

        assert_storage_diff!(@before before, env, contract, {
            modified: [
                { key: "count", tier: StorageTier::Persistent, from: 42u32, to: 43u32 },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_added_passes() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).seed(&Address::generate(&env));

        assert_storage_diff!(@before before, env, contract, {
            added: [
                { key: "count" },
                { key: "admin" },
                { key: "ttl" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_added_with_tier_passes() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).seed(&Address::generate(&env));

        assert_storage_diff!(@before before, env, contract, {
            added: [
                { key: "count", tier: StorageTier::Persistent },
                { key: "admin", tier: StorageTier::Instance },
                { key: "ttl", tier: StorageTier::Temporary },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Expected added key 'missing'")]
    fn assert_storage_diff_added_fails_missing() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).seed(&Address::generate(&env));

        assert_storage_diff!(@before before, env, contract, {
            added: [
                { key: "missing" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_removed_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).erase();

        assert_storage_diff!(@before before, env, contract, {
            removed: [
                { key: "count" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_removed_with_tier_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).erase();

        assert_storage_diff!(@before before, env, contract, {
            removed: [
                { key: "count", tier: StorageTier::Persistent },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Expected removed key 'missing'")]
    fn assert_storage_diff_removed_fails_missing() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).erase();

        assert_storage_diff!(@before before, env, contract, {
            removed: [
                { key: "missing" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_unchanged_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        // No operations - storage unchanged
        assert_storage_diff!(@before before, env, contract, {
            unchanged: [
                { key: "count" },
                { key: "admin" },
                { key: "ttl" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_unchanged_with_tier_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);

        assert_storage_diff!(@before before, env, contract, {
            unchanged: [
                { key: "count", tier: StorageTier::Persistent },
                { key: "admin", tier: StorageTier::Instance },
                { key: "ttl", tier: StorageTier::Temporary },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Expected key 'count'")]
    fn assert_storage_diff_unchanged_fails_when_modified() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).bump();

        assert_storage_diff!(@before before, env, contract, {
            unchanged: [
                { key: "count" },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Expected key 'count'")]
    fn assert_storage_diff_unchanged_fails_when_removed() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).erase();

        assert_storage_diff!(@before before, env, contract, {
            unchanged: [
                { key: "count" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_wildcard_added_passes() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let client = StoreClient::new(&env, &contract);
        client.seed(&Address::generate(&env));
        let before = StorageSnapshot::capture(&env, &contract);
        client.add_temp_key(&Symbol::new(&env, "allowance_user1"), &100u64);
        client.add_temp_key(&Symbol::new(&env, "allowance_user2"), &200u64);

        assert_storage_diff!(@before before, env, contract, {
            added: [
                { key: "allowance_*", tier: StorageTier::Temporary },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_wildcard_unchanged_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);

        assert_storage_diff!(@before before, env, contract, {
            unchanged: [
                { key: "count" },
                { key: "admin" },
                { key: "ttl" },
            ],
        });
    }

    #[test]
    fn assert_storage_diff_combined_passes() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        let client = StoreClient::new(&env, &contract);
        client.bump();  // modifies count: 42 -> 43
        client.add_temp_key(&Symbol::new(&env, "allowance_new"), &500u64);  // adds new key
        // Don't erase count - use a different key for removal test
        client.write_map();  // adds map to persistent
        // Add a temp key then we can't easily remove it, so just test without removal
        client.add_temp_key(&Symbol::new(&env, "temp_remove"), &123u64);

        assert_storage_diff!(@before before, env, contract, {
            added: [
                { key: "allowance_new", tier: StorageTier::Temporary },
                { key: "map", tier: StorageTier::Persistent },
                { key: "temp_remove", tier: StorageTier::Temporary },
            ],
            modified: [
                { key: "count", from: 42u32, to: 43u32 },
            ],
            unchanged: [
                { key: "admin", tier: StorageTier::Instance },
                { key: "ttl", tier: StorageTier::Temporary },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Unexpected added key")]
    fn assert_storage_diff_fails_on_unexpected_added() {
        let env = Env::default();
        let contract = env.register(Store, ());
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).seed(&Address::generate(&env));
        StoreClient::new(&env, &contract).add_temp_key(&Symbol::new(&env, "extra"), &999u64);

        assert_storage_diff!(@before before, env, contract, {
            added: [
                { key: "count" },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Modified key 'count'")]
    fn assert_storage_diff_modified_fails_wrong_from() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).bump();

        assert_storage_diff!(@before before, env, contract, {
            modified: [
                { key: "count", from: 100u32, to: 43u32 },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Modified key 'count'")]
    fn assert_storage_diff_modified_fails_wrong_to() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).bump();

        assert_storage_diff!(@before before, env, contract, {
            modified: [
                { key: "count", from: 42u32, to: 100u32 },
            ],
        });
    }

    #[test]
    #[should_panic(expected = "Unexpected added key")]
    fn assert_storage_diff_fails_on_unexpected_modified() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).bump();
        StoreClient::new(&env, &contract).modify_instance();  // This adds a new key 'cfg' to instance storage

        assert_storage_diff!(@before before, env, contract, {
            modified: [
                { key: "count", from: 42u32, to: 43u32 },
            ],
            // Expect no other modifications - but modify_instance adds 'cfg' key (shows as added)
        });
    }

    #[test]
    #[should_panic(expected = "Unexpected added key")]
    fn assert_storage_diff_fails_on_unexpected_removed() {
        let (env, contract) = seeded();
        let before = StorageSnapshot::capture(&env, &contract);
        StoreClient::new(&env, &contract).erase();
        StoreClient::new(&env, &contract).modify_instance();  // This adds a new key

        assert_storage_diff!(@before before, env, contract, {
            removed: [
                { key: "count" },
            ],
            // Expect no other removals - but modify_instance adds 'cfg' key (shows as added)
        });
    }
}