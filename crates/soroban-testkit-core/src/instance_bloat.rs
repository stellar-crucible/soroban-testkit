//! Detection of oversized and dynamically growing contract instance storage.

use std::collections::BTreeMap;

use soroban_sdk::{Address, Env};

use crate::storage::{StorageSnapshot, StorageTier};

pub const DEFAULT_INSTANCE_BLOAT_THRESHOLD: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceBloatWarningKind {
    ThresholdExceeded,
    CollectionGrowth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceBloatWarning {
    pub key: String,
    pub kind: InstanceBloatWarningKind,
    pub bytes: u64,
    pub previous_bytes: Option<u64>,
    pub threshold: u64,
}

impl std::fmt::Display for InstanceBloatWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            InstanceBloatWarningKind::ThresholdExceeded => write!(
                f,
                "instance storage bloat: key '{}' is {} bytes (threshold {} bytes)",
                self.key, self.bytes, self.threshold
            ),
            InstanceBloatWarningKind::CollectionGrowth => write!(
                f,
                "instance storage collection growth: key '{}' grew from {} to {} bytes",
                self.key,
                self.previous_bytes.unwrap_or(0),
                self.bytes
            ),
        }
    }
}

/// Watches instance storage and diagnoses byte and collection growth.
#[derive(Debug, Clone)]
pub struct InstanceBloatDetector {
    threshold: u64,
    baseline: Option<BTreeMap<String, u64>>,
    collection_baseline: Option<BTreeMap<String, u64>>,
    checkpoints: BTreeMap<String, BTreeMap<String, u64>>,
}

impl InstanceBloatDetector {
    pub fn new() -> Self {
        Self::with_threshold(DEFAULT_INSTANCE_BLOAT_THRESHOLD)
    }

    pub fn with_threshold(threshold: u64) -> Self {
        Self {
            threshold,
            baseline: None,
            collection_baseline: None,
            checkpoints: BTreeMap::new(),
        }
    }

    /// Capture the instance storage baseline.
    pub fn start(env: &Env, contract: &Address) -> Self {
        let mut detector = Self::new();
        detector.checkpoint(env, contract, "baseline");
        detector
    }

    pub fn threshold(&self) -> u64 {
        self.threshold
    }

    pub fn checkpoint(&mut self, env: &Env, contract: &Address, label: impl Into<String>) {
        let snapshot = StorageSnapshot::capture(env, contract);
        let values = instance_sizes(&snapshot);
        self.checkpoints.insert(label.into(), values.clone());
        if self.baseline.is_none() {
            self.baseline = Some(values.clone());
            self.collection_baseline = Some(collection_sizes(&snapshot));
        }
    }

    pub fn warnings(&self, env: &Env, contract: &Address) -> Vec<InstanceBloatWarning> {
        let snapshot = StorageSnapshot::capture(env, contract);
        let sizes = instance_sizes(&snapshot);
        let collection_sizes = collection_sizes(&snapshot);
        let mut warnings = Vec::new();

        for (key, bytes) in &sizes {
            if *bytes > self.threshold {
                warnings.push(InstanceBloatWarning {
                    key: key.clone(),
                    kind: InstanceBloatWarningKind::ThresholdExceeded,
                    bytes: *bytes,
                    previous_bytes: self
                        .baseline
                        .as_ref()
                        .and_then(|base| base.get(key))
                        .copied(),
                    threshold: self.threshold,
                });
            }
        }

        for (key, bytes) in &collection_sizes {
            let previous = self
                .collection_baseline
                .as_ref()
                .and_then(|base| base.get(key))
                .copied();
            if previous.is_some_and(|old| *bytes > old) {
                warnings.push(InstanceBloatWarning {
                    key: key.clone(),
                    kind: InstanceBloatWarningKind::CollectionGrowth,
                    bytes: *bytes,
                    previous_bytes: previous,
                    threshold: self.threshold,
                });
            }
        }

        warnings
    }

    pub fn total_bytes(&self, env: &Env, contract: &Address) -> u64 {
        let snapshot = StorageSnapshot::capture(env, contract);
        instance_sizes(&snapshot).values().copied().sum()
    }

    pub fn assert_no_bloat(&self, env: &Env, contract: &Address) {
        let warnings = self.warnings(env, contract);
        assert!(
            warnings.is_empty(),
            "instance storage bloat detected:\n{}",
            warnings
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    pub fn report(&self, env: &Env, contract: &Address) -> String {
        let warnings = self.warnings(env, contract);
        if warnings.is_empty() {
            return "no instance storage bloat detected".to_string();
        }
        warnings
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Default for InstanceBloatDetector {
    fn default() -> Self {
        Self::new()
    }
}

fn instance_sizes(snapshot: &StorageSnapshot) -> BTreeMap<String, u64> {
    snapshot
        .in_tier(StorageTier::Instance)
        .into_iter()
        .map(|entry| (entry.key.clone(), entry.bytes.len() as u64))
        .collect()
}

fn collection_sizes(snapshot: &StorageSnapshot) -> BTreeMap<String, u64> {
    snapshot
        .in_tier(StorageTier::Instance)
        .into_iter()
        .filter(|entry| entry.value.starts_with('[') || entry.value.starts_with('{'))
        .map(|entry| (entry.key.clone(), entry.bytes.len() as u64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, symbol_short, Env};

    #[contract]
    pub struct BloatContract;

    #[contractimpl]
    impl BloatContract {
        pub fn set_vec(env: Env, length: u32) {
            let values = (0..length).collect::<std::vec::Vec<u32>>();
            let mut soroban_values = soroban_sdk::Vec::new(&env);
            for value in values {
                soroban_values.push_back(value);
            }
            env.storage()
                .instance()
                .set(&symbol_short!("items"), &soroban_values);
        }

        pub fn set_scalar(env: Env) {
            env.storage().instance().set(&symbol_short!("admin"), &7u32);
        }
    }

    #[test]
    fn detects_instance_storage_above_threshold_and_collection_growth() {
        let env = Env::default();
        let contract = env.register(BloatContract, ());
        BloatContractClient::new(&env, &contract).set_vec(&1);

        let mut detector = InstanceBloatDetector::with_threshold(1);
        detector.checkpoint(&env, &contract, "small");
        BloatContractClient::new(&env, &contract).set_vec(&64);

        let warnings = detector.warnings(&env, &contract);
        assert!(warnings.iter().any(|warning| {
            warning.key == "items" && warning.kind == InstanceBloatWarningKind::ThresholdExceeded
        }));
        assert!(warnings.iter().any(|warning| {
            warning.key == "items" && warning.kind == InstanceBloatWarningKind::CollectionGrowth
        }));
    }

    #[test]
    fn scalar_instance_storage_reports_only_threshold_when_large() {
        let env = Env::default();
        let contract = env.register(BloatContract, ());
        BloatContractClient::new(&env, &contract).set_scalar();

        let detector = InstanceBloatDetector::with_threshold(1);
        let warnings = detector.warnings(&env, &contract);
        assert!(warnings.iter().any(|warning| {
            warning.key == "admin" && warning.kind == InstanceBloatWarningKind::ThresholdExceeded
        }));
    }

    #[test]
    fn report_names_the_specific_key() {
        let env = Env::default();
        let contract = env.register(BloatContract, ());
        BloatContractClient::new(&env, &contract).set_scalar();

        let report = InstanceBloatDetector::with_threshold(1).report(&env, &contract);
        assert!(report.contains("instance storage bloat"));
        assert!(report.contains("'admin'"));
    }
}
