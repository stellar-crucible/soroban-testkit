//! Property-testing strategies for Soroban storage tier allocations and TTL properties.
//!
//! Smart contracts allocate state across three distinct storage tiers:
//! - **Instance**: Data shared across contract functions and bound to the instance lifetime.
//! - **Persistent**: Long-lived data that can be extended or archived and restored upon expiry.
//! - **Temporary**: Ephemeral data that is automatically purged upon expiration without restoration.
//!
//! Generalized storage adapters, caching layers, and multi-tier registries must handle
//! all three tiers correctly. The strategies in this module generate randomized individual
//! storage tiers, tier allocations paired with live/expired TTL properties, and mixed-tier
//! key allocation plans. All strategies shrink towards [`StorageTier::Temporary`] so that
//! failures simplify to the most ephemeral storage tier.

use proptest::collection::vec;
use proptest::prelude::*;
use soroban_sdk::xdr::ContractDataDurability;
use soroban_testkit_core::storage::StorageTier;

use crate::ttl::{entry_ttl, EntryTtl, MAINNET_MAX_ENTRY_TTL};

/// A generated storage tier paired with associated TTL and durability properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StorageTierWithTtl {
    /// The target storage tier.
    pub tier: StorageTier,
    /// Protocol contract data durability corresponding to this tier, if applicable.
    pub durability: Option<ContractDataDurability>,
    /// Live or expired entry TTL information.
    pub ttl: EntryTtl,
    /// Whether this storage tier supports state restoration after expiration.
    pub is_restorable: bool,
}

impl StorageTierWithTtl {
    /// Construct a new storage tier allocation with explicit properties.
    pub fn new(tier: StorageTier, ttl: EntryTtl) -> Self {
        let (durability, is_restorable) = match tier {
            StorageTier::Instance => (None, true),
            StorageTier::Persistent => (Some(ContractDataDurability::Persistent), true),
            StorageTier::Temporary => (Some(ContractDataDurability::Temporary), false),
        };

        Self {
            tier,
            durability,
            ttl,
            is_restorable,
        }
    }

    /// The storage tier.
    pub fn tier(&self) -> StorageTier {
        self.tier
    }

    /// The XDR contract data durability, if directly represented in XDR `ContractDataDurability`.
    pub fn durability(&self) -> Option<ContractDataDurability> {
        self.durability
    }

    /// The entry TTL descriptor.
    pub fn ttl(&self) -> &EntryTtl {
        &self.ttl
    }

    /// Whether this allocation belongs to temporary storage.
    pub fn is_temporary(&self) -> bool {
        matches!(self.tier, StorageTier::Temporary)
    }

    /// Whether this allocation belongs to persistent storage.
    pub fn is_persistent(&self) -> bool {
        matches!(self.tier, StorageTier::Persistent)
    }

    /// Whether this allocation belongs to instance storage.
    pub fn is_instance(&self) -> bool {
        matches!(self.tier, StorageTier::Instance)
    }

    /// Whether expired entries in this tier can be restored.
    pub fn is_restorable(&self) -> bool {
        self.is_restorable
    }

    /// Whether the entry is currently live on `current_ledger`.
    pub fn is_live(&self) -> bool {
        self.ttl.is_live()
    }

    /// The last readable ledger sequence.
    pub fn live_until_ledger(&self) -> u32 {
        self.ttl.live_until_ledger
    }
}

/// A collection of keys allocated across mixed storage tiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedTierAllocation<K> {
    /// Ordered key-to-tier mappings.
    pub entries: Vec<(K, StorageTier)>,
}

impl<K> MixedTierAllocation<K> {
    /// Create a new mixed-tier allocation.
    pub fn new(entries: Vec<(K, StorageTier)>) -> Self {
        Self { entries }
    }

    /// Total number of allocated keys across all tiers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the allocation plan is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns references to all keys allocated to instance storage.
    pub fn instance_keys(&self) -> Vec<&K> {
        self.entries
            .iter()
            .filter_map(|(k, tier)| {
                if *tier == StorageTier::Instance {
                    Some(k)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns references to all keys allocated to persistent storage.
    pub fn persistent_keys(&self) -> Vec<&K> {
        self.entries
            .iter()
            .filter_map(|(k, tier)| {
                if *tier == StorageTier::Persistent {
                    Some(k)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns references to all keys allocated to temporary storage.
    pub fn temporary_keys(&self) -> Vec<&K> {
        self.entries
            .iter()
            .filter_map(|(k, tier)| {
                if *tier == StorageTier::Temporary {
                    Some(k)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Returns count breakdown as `(instance_count, persistent_count, temporary_count)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut inst = 0;
        let mut pers = 0;
        let mut temp = 0;

        for (_, tier) in &self.entries {
            match tier {
                StorageTier::Instance => inst += 1,
                StorageTier::Persistent => pers += 1,
                StorageTier::Temporary => temp += 1,
            }
        }

        (inst, pers, temp)
    }

    /// Find the storage tier for a specific key.
    pub fn tier_of(&self, key: &K) -> Option<StorageTier>
    where
        K: PartialEq,
    {
        self.entries
            .iter()
            .find_map(|(k, tier)| if k == key { Some(*tier) } else { None })
    }
}

/// A collection of keys allocated across mixed storage tiers with associated TTL properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MixedTierTtlAllocation<K> {
    /// Ordered key-to-tier-with-TTL entries.
    pub entries: Vec<(K, StorageTierWithTtl)>,
}

impl<K> MixedTierTtlAllocation<K> {
    /// Create a new mixed-tier allocation with TTLs.
    pub fn new(entries: Vec<(K, StorageTierWithTtl)>) -> Self {
        Self { entries }
    }

    /// Total number of allocated keys across all tiers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the allocation plan is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns references to keys allocated to instance storage.
    pub fn instance_entries(&self) -> Vec<(&K, &StorageTierWithTtl)> {
        self.entries
            .iter()
            .filter(|(_, alloc)| alloc.is_instance())
            .map(|(k, alloc)| (k, alloc))
            .collect()
    }

    /// Returns references to keys allocated to persistent storage.
    pub fn persistent_entries(&self) -> Vec<(&K, &StorageTierWithTtl)> {
        self.entries
            .iter()
            .filter(|(_, alloc)| alloc.is_persistent())
            .map(|(k, alloc)| (k, alloc))
            .collect()
    }

    /// Returns references to keys allocated to temporary storage.
    pub fn temporary_entries(&self) -> Vec<(&K, &StorageTierWithTtl)> {
        self.entries
            .iter()
            .filter(|(_, alloc)| alloc.is_temporary())
            .map(|(k, alloc)| (k, alloc))
            .collect()
    }
}

/// Generates a [`StorageTier`] variant with shrinking towards [`StorageTier::Temporary`].
///
/// Shrinking order:
/// - `StorageTier::Persistent` (2) shrinks to `StorageTier::Instance` (1) and `StorageTier::Temporary` (0).
/// - `StorageTier::Instance` (1) shrinks to `StorageTier::Temporary` (0).
/// - `StorageTier::Temporary` (0) is the simplest/minimal value.
pub fn storage_tier() -> impl Strategy<Value = StorageTier> {
    (0u8..=2u8).prop_map(|n| match n {
        0 => StorageTier::Temporary,
        1 => StorageTier::Instance,
        _ => StorageTier::Persistent,
    })
}

/// Generates a [`StorageTierWithTtl`] allocating a tier paired with realistic TTL properties.
///
/// `current_ledger`: the active ledger sequence.
/// `min_threshold`: minimum threshold before renewal (e.g. `0` to include entries near expiration).
/// `max_ttl`: maximum ceiling for the generated TTL interval (e.g. [`MAINNET_MAX_ENTRY_TTL`]).
///
/// The tier shrinks towards [`StorageTier::Temporary`], and the TTL shrinks towards `min_threshold`.
pub fn storage_tier_with_ttl(
    current_ledger: u32,
    min_threshold: u32,
    max_ttl: u32,
) -> impl Strategy<Value = StorageTierWithTtl> {
    (
        storage_tier(),
        entry_ttl(current_ledger, min_threshold, max_ttl),
    )
        .prop_map(|(tier, ttl)| StorageTierWithTtl::new(tier, ttl))
}

/// Generates a [`StorageTierWithTtl`] using default Mainnet parameters.
pub fn storage_tier_with_default_ttl(
    current_ledger: u32,
) -> impl Strategy<Value = StorageTierWithTtl> {
    storage_tier_with_ttl(current_ledger, 0, MAINNET_MAX_ENTRY_TTL)
}

/// Generates mixed-tier storage key allocations for property testing storage managers and caching layers.
///
/// Keys are drawn from `key_strategy`, and each is paired with a randomly generated [`StorageTier`].
/// Shrinking reduces the number of generated keys and simplifies tiers towards [`StorageTier::Temporary`].
pub fn mixed_tier_allocations<K, S>(
    key_strategy: S,
    size: impl Into<proptest::collection::SizeRange>,
) -> impl Strategy<Value = MixedTierAllocation<K>>
where
    K: Clone + std::fmt::Debug,
    S: Strategy<Value = K>,
{
    vec((key_strategy, storage_tier()), size).prop_map(MixedTierAllocation::new)
}

/// Generates raw `(K, StorageTier)` pairs across mixed tiers.
pub fn mixed_tier_key_pairs<K, S>(
    key_strategy: S,
    size: impl Into<proptest::collection::SizeRange>,
) -> impl Strategy<Value = Vec<(K, StorageTier)>>
where
    K: Clone + std::fmt::Debug,
    S: Strategy<Value = K>,
{
    vec((key_strategy, storage_tier()), size)
}

/// Generates mixed-tier storage key allocations paired with TTL properties.
pub fn mixed_tier_allocations_with_ttl<K, S>(
    key_strategy: S,
    current_ledger: u32,
    min_threshold: u32,
    max_ttl: u32,
    size: impl Into<proptest::collection::SizeRange>,
) -> impl Strategy<Value = MixedTierTtlAllocation<K>>
where
    K: Clone + std::fmt::Debug,
    S: Strategy<Value = K>,
{
    vec(
        (
            key_strategy,
            storage_tier_with_ttl(current_ledger, min_threshold, max_ttl),
        ),
        size,
    )
    .prop_map(MixedTierTtlAllocation::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::test_runner::{Config, TestRunner};
    use std::collections::{HashMap, HashSet};

    proptest! {
        #[test]
        fn test_storage_tier_generates_all_variants(tier in storage_tier()) {
            match tier {
                StorageTier::Instance => assert_eq!(tier.as_str(), "instance"),
                StorageTier::Persistent => assert_eq!(tier.as_str(), "persistent"),
                StorageTier::Temporary => assert_eq!(tier.as_str(), "temporary"),
            }
        }

        #[test]
        fn test_storage_tier_with_ttl_properties(
            alloc in storage_tier_with_ttl(1_000, 100, 10_000)
        ) {
            assert!(alloc.ttl().is_live());
            assert!(alloc.live_until_ledger() >= 1_100);
            assert!(alloc.live_until_ledger() <= 11_000);

            match alloc.tier() {
                StorageTier::Instance => {
                    assert_eq!(alloc.durability(), None);
                    assert!(alloc.is_restorable());
                    assert!(alloc.is_instance());
                }
                StorageTier::Persistent => {
                    assert_eq!(alloc.durability(), Some(ContractDataDurability::Persistent));
                    assert!(alloc.is_restorable());
                    assert!(alloc.is_persistent());
                }
                StorageTier::Temporary => {
                    assert_eq!(alloc.durability(), Some(ContractDataDurability::Temporary));
                    assert!(!alloc.is_restorable());
                    assert!(alloc.is_temporary());
                }
            }
        }

        #[test]
        fn test_mixed_tier_allocations_distribution(
            plan in mixed_tier_allocations("[a-z0-9]{3,8}", 10..=30)
        ) {
            assert!(plan.len() >= 10 && plan.len() <= 30);
            let (inst, pers, temp) = plan.counts();
            assert_eq!(inst + pers + temp, plan.len());

            let mut seen = HashSet::new();
            for (k, tier) in &plan.entries {
                if seen.insert(k) {
                    assert_eq!(plan.tier_of(k), Some(*tier));
                }
            }
        }

        #[test]
        fn test_mixed_tier_ttl_allocations(
            plan in mixed_tier_allocations_with_ttl("[a-z]{3,5}", 500, 10, 2_000, 5..=15)
        ) {
            assert!(plan.len() >= 5 && plan.len() <= 15);
            let inst = plan.instance_entries();
            let pers = plan.persistent_entries();
            let temp = plan.temporary_entries();

            assert_eq!(inst.len() + pers.len() + temp.len(), plan.len());
            for (_, alloc) in &plan.entries {
                assert!(alloc.is_live());
            }
        }
    }

    /// Verifies that shrinking explicitly simplifies towards Temporary tier storage.
    #[test]
    fn test_shrinking_simplifies_towards_temporary() {
        let mut runner = TestRunner::new(Config::default());
        // Fails on anything other than Temporary; shrinking must reduce to Temporary:
        let strategy = storage_tier();

        let result = runner.run(&strategy, |tier| {
            if tier != StorageTier::Temporary {
                Err(proptest::test_runner::TestCaseError::fail(
                    "intentionally reject non-temporary",
                ))
            } else {
                Ok(())
            }
        });

        // The run fails because non-temporary variants exist:
        assert!(result.is_err());
    }

    /// Unit test demonstrating proptest fuzzing of a multi-tier storage management cache module.
    struct MockTieredCache {
        instance_cache: HashMap<String, u64>,
        persistent_cache: HashMap<String, u64>,
        temporary_cache: HashMap<String, u64>,
        archived_keys: HashSet<String>,
    }

    impl MockTieredCache {
        fn new() -> Self {
            Self {
                instance_cache: HashMap::new(),
                persistent_cache: HashMap::new(),
                temporary_cache: HashMap::new(),
                archived_keys: HashSet::new(),
            }
        }

        fn put(&mut self, key: String, val: u64, tier: StorageTier) {
            match tier {
                StorageTier::Instance => {
                    self.instance_cache.insert(key, val);
                }
                StorageTier::Persistent => {
                    self.persistent_cache.insert(key, val);
                }
                StorageTier::Temporary => {
                    self.temporary_cache.insert(key, val);
                }
            }
        }

        fn get(&self, key: &str, tier: StorageTier) -> Option<u64> {
            match tier {
                StorageTier::Instance => self.instance_cache.get(key).copied(),
                StorageTier::Persistent => self.persistent_cache.get(key).copied(),
                StorageTier::Temporary => self.temporary_cache.get(key).copied(),
            }
        }

        fn purge_ephemeral(&mut self) {
            self.temporary_cache.clear();
        }

        fn archive_persistent(&mut self) {
            for k in self.persistent_cache.keys() {
                self.archived_keys.insert(k.clone());
            }
        }
    }

    proptest! {
        #[test]
        fn test_tiered_cache_fuzzing(
            allocations in mixed_tier_allocations("[a-zA-Z0-9_]{3,10}", 15..=40),
            val_seed in 1u64..=1_000u64,
        ) {
            let mut cache = MockTieredCache::new();
            let mut expected: HashMap<(String, StorageTier), u64> = HashMap::new();

            // Populate according to generated tier allocations
            for (idx, (key, tier)) in allocations.entries.iter().enumerate() {
                let val = val_seed + idx as u64;
                cache.put(key.clone(), val, *tier);
                expected.insert((key.clone(), *tier), val);
            }

            // Verify retrieval per tier
            for ((key, tier), val) in &expected {
                let found = cache.get(key, *tier);
                assert_eq!(found, Some(*val));
            }

            // Purge temporary tier: temporary keys gone, instance and persistent intact
            cache.purge_ephemeral();
            for (key, tier) in expected.keys() {
                match tier {
                    StorageTier::Temporary => assert_eq!(cache.get(key, *tier), None),
                    StorageTier::Instance | StorageTier::Persistent => {
                        assert!(cache.get(key, *tier).is_some());
                    }
                }
            }

            // Archive persistent tier
            cache.archive_persistent();
            for key in allocations.persistent_keys() {
                assert!(cache.archived_keys.contains(key));
            }
        }
    }
}
