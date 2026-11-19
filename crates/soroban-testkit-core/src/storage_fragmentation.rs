//! Contract storage fragmentation and serialization overhead analyzer.
//!
//! Storing contract state across numerous small independent storage entries incurs
//! substantial serialization and ledger key overhead in XDR compared to packed
//! composite structures. [`StorageFragmentationAnalyzer`] measures the key-to-value
//! byte ratio across instance, persistent, and temporary storage tiers, detects
//! fragmented keys, identifies consolidation candidates, and estimates byte and
//! Stroop rent savings.

use std::collections::BTreeMap;

use soroban_sdk::{
    xdr::{ContractDataDurability, LedgerEntryData, LedgerKey, Limits, ScVal, WriteXdr},
    Address, Env,
};

use crate::storage::{contract_hash, contract_hash_of, render, StorageSnapshot, StorageTier};

/// Default rent cost in Stroops per byte for projected savings calculations.
///
/// On Stellar Mainnet, rent rates vary by ledger lifetime; 100 Stroops/byte
/// is a representative baseline for medium-term storage holding costs.
pub const DEFAULT_STROOP_RATE_PER_BYTE: u64 = 100;

/// Default threshold where key bytes / total bytes ratio is considered high overhead.
pub const DEFAULT_HIGH_OVERHEAD_THRESHOLD: f64 = 0.40;

/// Overhead and serialization metrics for a single storage entry.
#[derive(Debug, Clone, PartialEq)]
pub struct StorageEntryMetrics {
    /// Rendered representation of the entry's key.
    pub key: String,
    /// Storage tier where the entry lives.
    pub tier: StorageTier,
    /// Serialized XDR size of the entry key in bytes.
    pub key_bytes: u64,
    /// Serialized XDR size of the entry value in bytes.
    pub val_bytes: u64,
    /// Total serialized size (`key_bytes + val_bytes`).
    pub total_bytes: u64,
    /// Proportion of the entry consumed by key metadata (`key_bytes / total_bytes`).
    pub overhead_ratio: f64,
    /// Ratio of key size compared to value size (`key_bytes / val_bytes`).
    pub key_to_value_ratio: f64,
    /// Whether this entry exceeds the configured high-overhead threshold.
    pub is_high_overhead: bool,
}

/// Aggregate metrics for a single storage tier.
#[derive(Debug, Clone, PartialEq)]
pub struct TierFragmentationSummary {
    /// The storage tier.
    pub tier: StorageTier,
    /// Number of entries stored in this tier.
    pub entry_count: usize,
    /// Sum of key bytes in this tier.
    pub total_key_bytes: u64,
    /// Sum of value bytes in this tier.
    pub total_val_bytes: u64,
    /// Sum of key + value bytes in this tier.
    pub total_bytes: u64,
    /// Overall key overhead ratio for this tier (`total_key_bytes / total_bytes`).
    pub overhead_ratio: f64,
}

/// A cluster of related storage keys eligible for consolidation into a composite struct.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidationOpportunity {
    /// Common prefix or domain identifier uniting the entries.
    pub group_prefix: String,
    /// Storage tier where these entries reside.
    pub tier: StorageTier,
    /// The individual storage keys included in this group.
    pub entries: Vec<String>,
    /// Number of individual entries currently maintaining separate keys.
    pub current_entries_count: usize,
    /// Current total serialized size in bytes across all entries in the group.
    pub current_total_bytes: u64,
    /// Projected serialized size if merged into a single composite struct.
    pub projected_consolidated_bytes: u64,
    /// Estimated byte savings from eliminating redundant keys and ledger headers.
    pub estimated_byte_savings: u64,
    /// Projected Stroop rent savings based on the analyzer's configured rate.
    pub projected_stroop_savings: u64,
}

/// Comprehensive report produced by [`StorageFragmentationAnalyzer`].
#[derive(Debug, Clone, PartialEq)]
pub struct FragmentationReport {
    /// Metrics for every examined storage entry.
    pub entries: Vec<StorageEntryMetrics>,
    /// Per-tier breakdown of fragmentation metrics.
    pub tier_summaries: BTreeMap<StorageTier, TierFragmentationSummary>,
    /// Detected opportunities to consolidate fragmented entries.
    pub consolidation_opportunities: Vec<ConsolidationOpportunity>,
    /// Total key bytes across all active entries.
    pub total_key_bytes: u64,
    /// Total value bytes across all active entries.
    pub total_val_bytes: u64,
    /// Grand total of serialized bytes (`total_key_bytes + total_val_bytes`).
    pub total_bytes: u64,
    /// Grand total key overhead ratio (`total_key_bytes / total_bytes`).
    pub overall_overhead_ratio: f64,
    /// Count of entries flagged as having high serialization overhead.
    pub high_overhead_count: usize,
}

impl FragmentationReport {
    /// Returns entries exceeding the high-overhead threshold.
    pub fn high_overhead_entries(&self) -> Vec<&StorageEntryMetrics> {
        self.entries
            .iter()
            .filter(|entry| entry.is_high_overhead)
            .collect()
    }

    /// Whether any entry was flagged for high overhead.
    pub fn has_high_overhead(&self) -> bool {
        self.high_overhead_count > 0
    }

    /// Sum of all estimated byte savings across identified consolidation opportunities.
    pub fn total_projected_byte_savings(&self) -> u64 {
        self.consolidation_opportunities
            .iter()
            .map(|opp| opp.estimated_byte_savings)
            .sum()
    }

    /// Sum of all projected Stroop savings across identified consolidation opportunities.
    pub fn total_projected_stroop_savings(&self) -> u64 {
        self.consolidation_opportunities
            .iter()
            .map(|opp| opp.projected_stroop_savings)
            .sum()
    }

    /// Format the analysis as an actionable Markdown optimization report.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Storage Fragmentation & Serialization Overhead Report\n\n");

        md.push_str("## Executive Summary\n\n");
        md.push_str(&format!(
            "- **Total Storage Entries**: {}\n",
            self.entries.len()
        ));
        md.push_str(&format!(
            "- **Total Serialized Bytes**: {} B (Key: {} B, Value: {} B)\n",
            self.total_bytes, self.total_key_bytes, self.total_val_bytes
        ));
        md.push_str(&format!(
            "- **Overall Key Overhead Ratio**: {:.2}%\n",
            self.overall_overhead_ratio * 100.0
        ));
        md.push_str(&format!(
            "- **High Overhead Entries Flagged**: {}\n",
            self.high_overhead_count
        ));
        md.push_str(&format!(
            "- **Potential Consolidation Savings**: {} B (~{} Stroops)\n\n",
            self.total_projected_byte_savings(),
            self.total_projected_stroop_savings()
        ));

        md.push_str("## Tier Breakdown\n\n");
        md.push_str(
            "| Tier | Entries | Key Bytes | Value Bytes | Total Bytes | Overhead Ratio |\n",
        );
        md.push_str(
            "|------|---------|-----------|-------------|-------------|----------------|\n",
        );
        for tier in &[
            StorageTier::Instance,
            StorageTier::Persistent,
            StorageTier::Temporary,
        ] {
            if let Some(summary) = self.tier_summaries.get(tier) {
                md.push_str(&format!(
                    "| `{}` | {} | {} B | {} B | {} B | {:.1}% |\n",
                    summary.tier.as_str(),
                    summary.entry_count,
                    summary.total_key_bytes,
                    summary.total_val_bytes,
                    summary.total_bytes,
                    summary.overhead_ratio * 100.0
                ));
            } else {
                md.push_str(&format!(
                    "| `{}` | 0 | 0 B | 0 B | 0 B | 0.0% |\n",
                    tier.as_str()
                ));
            }
        }
        md.push('\n');

        let high_overhead = self.high_overhead_entries();
        if !high_overhead.is_empty() {
            md.push_str("## High Overhead Entries\n\n");
            md.push_str(
                "Entries where key metadata consumes a disproportionate share of stored bytes:\n\n",
            );
            md.push_str("| Key | Tier | Key Size | Value Size | Overhead Ratio | Status |\n");
            md.push_str("|-----|------|----------|------------|----------------|--------|\n");
            for entry in high_overhead {
                md.push_str(&format!(
                    "| `{}` | `{}` | {} B | {} B | {:.1}% | ⚠️ Flagged |\n",
                    entry.key,
                    entry.tier.as_str(),
                    entry.key_bytes,
                    entry.val_bytes,
                    entry.overhead_ratio * 100.0
                ));
            }
            md.push('\n');
        }

        if !self.consolidation_opportunities.is_empty() {
            md.push_str("## Consolidation Opportunities\n\n");
            md.push_str("Consolidating related scalar entries into packed composite structs eliminates redundant keys:\n\n");
            md.push_str("| Group / Prefix | Tier | Current Entries | Current Bytes | Projected Bytes | Byte Savings | Est. Stroop Savings |\n");
            md.push_str("|----------------|------|-----------------|---------------|-----------------|--------------|---------------------|\n");
            for opp in &self.consolidation_opportunities {
                md.push_str(&format!(
                    "| `{}` | `{}` | {} | {} B | {} B | **{} B** | **{} Stroops** |\n",
                    opp.group_prefix,
                    opp.tier.as_str(),
                    opp.current_entries_count,
                    opp.current_total_bytes,
                    opp.projected_consolidated_bytes,
                    opp.estimated_byte_savings,
                    opp.projected_stroop_savings
                ));
            }
            md.push('\n');
        }

        md.push_str("## Recommendations\n\n");
        if self.consolidation_opportunities.is_empty() && self.high_overhead_count == 0 {
            md.push_str("✅ Storage layout is clean with minimal key overhead. No immediate consolidation required.\n");
        } else {
            md.push_str("1. **Pack Related Fields**: Group multi-entry entities into single composite structs (`#[contracttype] struct UserData { ... }`).\n");
            md.push_str("2. **Minimize Key Enum Discriminants**: Prefer compact symbol or integer identifiers over verbose strings.\n");
            md.push_str("3. **Leverage Instance Storage for Config**: Move singleton settings from persistent entries into instance storage.\n");
        }

        md
    }
}

/// Analyzer for storage fragmentation and key serialization overhead.
#[derive(Debug, Clone)]
pub struct StorageFragmentationAnalyzer {
    overhead_threshold: f64,
    stroop_rate_per_byte: u64,
}

impl Default for StorageFragmentationAnalyzer {
    fn default() -> Self {
        Self {
            overhead_threshold: DEFAULT_HIGH_OVERHEAD_THRESHOLD,
            stroop_rate_per_byte: DEFAULT_STROOP_RATE_PER_BYTE,
        }
    }
}

impl StorageFragmentationAnalyzer {
    /// Create a new analyzer with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set custom overhead threshold (between 0.0 and 1.0) for flagging entries.
    pub fn with_overhead_threshold(mut self, threshold: f64) -> Self {
        self.overhead_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set projected Stroop rent cost per byte.
    pub fn with_stroop_rate_per_byte(mut self, rate: u64) -> Self {
        self.stroop_rate_per_byte = rate;
        self
    }

    /// Analyze all active storage entries for `contract` in `env`.
    pub fn analyze(&self, env: &Env, contract: &Address) -> FragmentationReport {
        let Some(wanted) = contract_hash_of(env, contract) else {
            return self.empty_report();
        };

        let mut raw_items = Vec::new();

        for (key, (entry, _)) in env.to_ledger_snapshot().ledger_entries.iter() {
            let LedgerKey::ContractData(lookup) = &**key else {
                continue;
            };
            if contract_hash(&lookup.contract) != Some(wanted) {
                continue;
            }
            let LedgerEntryData::ContractData(stored) = &entry.data else {
                continue;
            };

            match &stored.key {
                ScVal::LedgerKeyContractInstance => {
                    let ScVal::ContractInstance(instance) = &stored.val else {
                        continue;
                    };
                    let Some(storage) = &instance.storage else {
                        continue;
                    };
                    for item in storage.0.iter() {
                        let k_bytes = xdr_len(&item.key);
                        let v_bytes = xdr_len(&item.val);
                        raw_items.push((
                            StorageTier::Instance,
                            render(&item.key),
                            k_bytes,
                            v_bytes,
                        ));
                    }
                }
                stored_key => {
                    let tier = match stored.durability {
                        ContractDataDurability::Temporary => StorageTier::Temporary,
                        ContractDataDurability::Persistent => StorageTier::Persistent,
                    };
                    let k_bytes = xdr_len(stored_key);
                    let v_bytes = xdr_len(&stored.val);
                    raw_items.push((tier, render(stored_key), k_bytes, v_bytes));
                }
            }
        }

        self.build_report(raw_items)
    }

    /// Analyze storage from an existing [`StorageSnapshot`].
    pub fn analyze_snapshot(&self, snapshot: &StorageSnapshot) -> FragmentationReport {
        let raw_items = snapshot
            .entries()
            .iter()
            .map(|entry| {
                let k_bytes = entry.key.len() as u64;
                let v_bytes = entry.value.len() as u64;
                (entry.tier, entry.key.clone(), k_bytes, v_bytes)
            })
            .collect();
        self.build_report(raw_items)
    }

    /// Analyze a list of raw items `(StorageTier, ScVal key, ScVal val)`.
    pub fn analyze_scvals(&self, items: &[(StorageTier, ScVal, ScVal)]) -> FragmentationReport {
        let raw_items = items
            .iter()
            .map(|(tier, key, val)| {
                let k_bytes = xdr_len(key);
                let v_bytes = xdr_len(val);
                (*tier, render(key), k_bytes, v_bytes)
            })
            .collect();
        self.build_report(raw_items)
    }

    /// Construct the full [`FragmentationReport`] from raw key/value byte metrics.
    fn build_report(&self, items: Vec<(StorageTier, String, u64, u64)>) -> FragmentationReport {
        let mut entries = Vec::with_capacity(items.len());
        let mut tier_stats: BTreeMap<StorageTier, (usize, u64, u64)> = BTreeMap::new();

        let mut total_key_bytes = 0u64;
        let mut total_val_bytes = 0u64;
        let mut high_overhead_count = 0usize;

        for (tier, key, key_bytes, val_bytes) in items {
            let total_bytes = key_bytes.saturating_add(val_bytes);
            let overhead_ratio = if total_bytes > 0 {
                key_bytes as f64 / total_bytes as f64
            } else {
                0.0
            };
            let key_to_value_ratio = if val_bytes > 0 {
                key_bytes as f64 / val_bytes as f64
            } else {
                key_bytes as f64
            };
            let is_high_overhead = overhead_ratio >= self.overhead_threshold;
            if is_high_overhead {
                high_overhead_count += 1;
            }

            total_key_bytes = total_key_bytes.saturating_add(key_bytes);
            total_val_bytes = total_val_bytes.saturating_add(val_bytes);

            let stat = tier_stats.entry(tier).or_insert((0, 0, 0));
            stat.0 += 1;
            stat.1 = stat.1.saturating_add(key_bytes);
            stat.2 = stat.2.saturating_add(val_bytes);

            entries.push(StorageEntryMetrics {
                key,
                tier,
                key_bytes,
                val_bytes,
                total_bytes,
                overhead_ratio,
                key_to_value_ratio,
                is_high_overhead,
            });
        }

        let total_bytes = total_key_bytes.saturating_add(total_val_bytes);
        let overall_overhead_ratio = if total_bytes > 0 {
            total_key_bytes as f64 / total_bytes as f64
        } else {
            0.0
        };

        let mut tier_summaries = BTreeMap::new();
        for (tier, (count, k_bytes, v_bytes)) in tier_stats {
            let t_bytes = k_bytes.saturating_add(v_bytes);
            let overhead_ratio = if t_bytes > 0 {
                k_bytes as f64 / t_bytes as f64
            } else {
                0.0
            };
            tier_summaries.insert(
                tier,
                TierFragmentationSummary {
                    tier,
                    entry_count: count,
                    total_key_bytes: k_bytes,
                    total_val_bytes: v_bytes,
                    total_bytes: t_bytes,
                    overhead_ratio,
                },
            );
        }

        let consolidation_opportunities = self.detect_consolidation(&entries);

        FragmentationReport {
            entries,
            tier_summaries,
            consolidation_opportunities,
            total_key_bytes,
            total_val_bytes,
            total_bytes,
            overall_overhead_ratio,
            high_overhead_count,
        }
    }

    /// Detect groups of fragmented keys that share common prefixes or namespaces.
    fn detect_consolidation(
        &self,
        entries: &[StorageEntryMetrics],
    ) -> Vec<ConsolidationOpportunity> {
        let mut prefix_groups: BTreeMap<(StorageTier, String), Vec<&StorageEntryMetrics>> =
            BTreeMap::new();

        for entry in entries {
            if let Some(prefix) = extract_prefix(&entry.key) {
                prefix_groups
                    .entry((entry.tier, prefix))
                    .or_default()
                    .push(entry);
            }
        }

        let mut opportunities = Vec::new();

        for ((tier, prefix), group_entries) in prefix_groups {
            if group_entries.len() >= 2 {
                let current_entries_count = group_entries.len();
                let current_total_bytes: u64 = group_entries.iter().map(|e| e.total_bytes).sum();

                // Consolidated representation:
                // One key representing the group prefix (average key bytes + small header)
                // Plus combined values with minor struct overhead (~4 bytes per field).
                let single_key_bytes = group_entries
                    .first()
                    .map(|e| e.key_bytes.max(prefix.len() as u64 + 4))
                    .unwrap_or(16);
                let combined_val_bytes: u64 = group_entries
                    .iter()
                    .map(|e| e.val_bytes.saturating_add(4))
                    .sum();
                let projected_consolidated_bytes =
                    single_key_bytes.saturating_add(combined_val_bytes);

                let estimated_byte_savings =
                    current_total_bytes.saturating_sub(projected_consolidated_bytes);

                if estimated_byte_savings > 0 {
                    let projected_stroop_savings =
                        estimated_byte_savings.saturating_mul(self.stroop_rate_per_byte);

                    opportunities.push(ConsolidationOpportunity {
                        group_prefix: prefix,
                        tier,
                        entries: group_entries.iter().map(|e| e.key.clone()).collect(),
                        current_entries_count,
                        current_total_bytes,
                        projected_consolidated_bytes,
                        estimated_byte_savings,
                        projected_stroop_savings,
                    });
                }
            }
        }

        opportunities
    }

    fn empty_report(&self) -> FragmentationReport {
        FragmentationReport {
            entries: Vec::new(),
            tier_summaries: BTreeMap::new(),
            consolidation_opportunities: Vec::new(),
            total_key_bytes: 0,
            total_val_bytes: 0,
            total_bytes: 0,
            overall_overhead_ratio: 0.0,
            high_overhead_count: 0,
        }
    }
}

/// Helper to serialize an XDR value and obtain its byte length.
fn xdr_len(value: &impl WriteXdr) -> u64 {
    value
        .to_xdr(Limits::none())
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(0)
}

/// Extract common prefix identifier from key separators ("::", ":", "_", "/", ".").
fn extract_prefix(key: &str) -> Option<String> {
    for sep in &["::", ":", "_", "/", "."] {
        if let Some((prefix, _)) = key.split_once(sep) {
            if !prefix.is_empty() {
                return Some(prefix.to_string());
            }
        }
    }
    // Check tuple-like strings e.g. "[DataKey, User, ...]"
    if key.starts_with('[') && key.contains(',') {
        if let Some((first, _)) = key[1..].split_once(',') {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contractimpl, symbol_short,
        testutils::Address as _,
        xdr::{ScSymbol, StringM},
        Address, Env,
    };

    #[contract]
    pub struct FragmentationTestContract;

    #[contractimpl]
    impl FragmentationTestContract {
        pub fn init(env: Env) {
            // Write fragmented individual keys in persistent storage
            env.storage()
                .persistent()
                .set(&symbol_short!("u_bal"), &1000u64);
            env.storage()
                .persistent()
                .set(&symbol_short!("u_seq"), &1u32);
            env.storage()
                .persistent()
                .set(&symbol_short!("u_role"), &symbol_short!("admin"));

            // Write instance storage
            env.storage().instance().set(&symbol_short!("admin"), &1u32);

            // Write temporary storage
            env.storage()
                .temporary()
                .set(&symbol_short!("sess_tok"), &1u32);
        }
    }

    #[test]
    fn test_analyzer_detects_overhead_and_tier_summaries() {
        let env = Env::default();
        let contract_id = env.register(FragmentationTestContract, ());
        let client = FragmentationTestContractClient::new(&env, &contract_id);
        client.init();

        let analyzer = StorageFragmentationAnalyzer::new().with_overhead_threshold(0.35);
        let report = analyzer.analyze(&env, &contract_id);

        assert!(!report.entries.is_empty());
        assert!(report.total_bytes > 0);
        assert!(report.total_key_bytes > 0);
        assert!(report.total_val_bytes > 0);
        assert!(report.overall_overhead_ratio > 0.0);

        // Persistent tier check
        let persistent = report
            .tier_summaries
            .get(&StorageTier::Persistent)
            .expect("persistent tier summary");
        assert_eq!(persistent.entry_count, 3);
        assert!(persistent.total_key_bytes > 0);

        // Instance tier check
        let instance = report
            .tier_summaries
            .get(&StorageTier::Instance)
            .expect("instance tier summary");
        assert_eq!(instance.entry_count, 1);

        // Markdown report verification
        let md = report.to_markdown();
        assert!(md.contains("# Storage Fragmentation & Serialization Overhead Report"));
        assert!(md.contains("## Tier Breakdown"));
        assert!(md.contains("`persistent`"));
        assert!(md.contains("`instance`"));
    }

    #[test]
    fn test_consolidation_detection_and_savings() {
        let analyzer = StorageFragmentationAnalyzer::new()
            .with_overhead_threshold(0.3)
            .with_stroop_rate_per_byte(200);

        let make_sym = |s: &str| ScVal::Symbol(ScSymbol(StringM::try_from(s).unwrap()));

        // Mock items with common prefix "user_*"
        let items = vec![
            (
                StorageTier::Persistent,
                make_sym("user_bal"),
                ScVal::U64(500),
            ),
            (StorageTier::Persistent, make_sym("user_seq"), ScVal::U32(1)),
            (
                StorageTier::Persistent,
                make_sym("user_tag"),
                ScVal::U32(42),
            ),
        ];

        let report = analyzer.analyze_scvals(&items);

        assert_eq!(report.entries.len(), 3);
        assert_eq!(report.consolidation_opportunities.len(), 1);

        let opp = &report.consolidation_opportunities[0];
        assert_eq!(opp.group_prefix, "user");
        assert_eq!(opp.current_entries_count, 3);
        assert!(opp.estimated_byte_savings > 0);
        assert_eq!(
            opp.projected_stroop_savings,
            opp.estimated_byte_savings * 200
        );

        let md = report.to_markdown();
        assert!(md.contains("## Consolidation Opportunities"));
        assert!(md.contains("`user`"));
    }

    #[test]
    fn test_empty_contract_yields_empty_report() {
        let env = Env::default();
        let uninit_addr = Address::generate(&env);

        let analyzer = StorageFragmentationAnalyzer::new();
        let report = analyzer.analyze(&env, &uninit_addr);

        assert_eq!(report.entries.len(), 0);
        assert_eq!(report.total_bytes, 0);
        assert_eq!(report.overall_overhead_ratio, 0.0);
        assert!(!report.has_high_overhead());
        assert_eq!(report.total_projected_byte_savings(), 0);
    }
}
