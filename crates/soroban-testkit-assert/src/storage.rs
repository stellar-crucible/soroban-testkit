//! Storage tier entry count quota assertions.
//!
//! Smart contract state must be partitioned appropriately across instance,
//! persistent, and temporary storage tiers. [`StorageQuotaMatcher`] and the
//! [`assert_storage_tier_count!`] macro enforce entry count quotas per tier,
//! validating exact counts or upper bounds (`max_*`), and format an informative
//! breakdown table listing all keys in each tier whenever a quota is breached.

use std::collections::BTreeMap;
use std::fmt;

use soroban_sdk::{Address, Env};
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};

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
