//! Memory footprint and allocation profiler for Soroban contract invocations.
//!
//! The Soroban host enforces strict memory limits alongside CPU limits. Contracts with nested
//! collections or recursive data structures frequently exhaust host memory on-chain despite
//! passing CPU metering locally.
//!
//! [`MemoryProfiler`] captures memory allocation deltas, measures memory bytes metered per
//! execution step via `env.budget().memory_bytes_cost()`, tracks host object allocations, identifies
//! peak allocation points, and generates structured text and JSON reports with configurable threshold warnings.

use serde::{Deserialize, Serialize};
use soroban_sdk::testutils::budget::ContractCostType;
use soroban_sdk::Env;
use std::fmt;

/// Metrics captured for a single profiled execution step or contract call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStep {
    /// Human-readable label for this execution step.
    pub label: String,
    /// Memory bytes metered before the step was executed.
    pub memory_bytes_before: u64,
    /// Memory bytes metered after the step was executed.
    pub memory_bytes_after: u64,
    /// Net increase in metered memory bytes during this step (`after - before`).
    pub memory_delta_bytes: u64,
    /// Peak modelled host memory in use after this step.
    pub peak_live_bytes: u64,
    /// Number of `MemAlloc` host allocation operations charged during this step.
    pub host_allocations: u64,
    /// Total bytes of host memory allocated across charges during this step.
    pub host_allocated_bytes: u64,
    /// Total bytes copied between host objects during this step.
    pub host_copied_bytes: u64,
}

impl MemoryStep {
    /// Formats the memory delta as a human-readable string (e.g. `12.5 KB`).
    pub fn human_delta(&self) -> String {
        crate::memory::human_bytes(self.memory_delta_bytes)
    }

    /// Formats the memory after execution as a human-readable string.
    pub fn human_after(&self) -> String {
        crate::memory::human_bytes(self.memory_bytes_after)
    }
}

/// A structured report aggregating metrics across all profiled steps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryProfileReport {
    /// All execution steps recorded in invocation order.
    pub steps: Vec<MemoryStep>,
    /// Highest memory bytes cost observed across all steps.
    pub peak_memory_bytes: u64,
    /// Sum of all memory deltas across recorded steps.
    pub total_memory_delta: u64,
    /// Total number of host memory allocations across all steps.
    pub total_host_allocations: u64,
    /// Total host memory allocated across all steps.
    pub total_host_allocated_bytes: u64,
    /// Configured threshold in bytes that triggers regression/exhaustion warnings.
    pub warning_threshold_bytes: Option<u64>,
    /// List of diagnostic warning messages for steps nearing or exceeding the threshold.
    pub warnings: Vec<String>,
}

impl MemoryProfileReport {
    /// Returns true if any warning was generated.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Returns a short summary of peak consumption and warning counts.
    pub fn summary(&self) -> String {
        format!(
            "MemoryProfile: {} steps, peak {} ({} warnings)",
            self.steps.len(),
            crate::memory::human_bytes(self.peak_memory_bytes),
            self.warnings.len()
        )
    }

    /// Serializes this report to a formatted JSON string.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

impl fmt::Display for MemoryProfileReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "=== Soroban Memory Profile Report ===")?;
        writeln!(
            f,
            "Peak Memory: {} ({} bytes)",
            crate::memory::human_bytes(self.peak_memory_bytes),
            self.peak_memory_bytes
        )?;
        writeln!(
            f,
            "Total Delta: {} ({} bytes)",
            crate::memory::human_bytes(self.total_memory_delta),
            self.total_memory_delta
        )?;
        writeln!(
            f,
            "Total Host Allocations: {} ({} allocated)",
            self.total_host_allocations,
            crate::memory::human_bytes(self.total_host_allocated_bytes)
        )?;

        if let Some(threshold) = self.warning_threshold_bytes {
            writeln!(
                f,
                "Warning Threshold: {} ({} bytes)",
                crate::memory::human_bytes(threshold),
                threshold
            )?;
        }

        writeln!(f)?;
        writeln!(
            f,
            "{:<20} {:>12} {:>12} {:>12} {:>10} {:>12}",
            "Step", "Before", "After", "Delta", "Allocs", "Alloc Bytes"
        )?;
        writeln!(
            f,
            "{:-<20} {:-<12} {:-<12} {:-<12} {:-<10} {:-<12}",
            "", "", "", "", "", ""
        )?;

        for step in &self.steps {
            writeln!(
                f,
                "{:<20} {:>12} {:>12} {:>12} {:>10} {:>12}",
                step.label,
                step.memory_bytes_before,
                step.memory_bytes_after,
                step.memory_delta_bytes,
                step.host_allocations,
                step.host_allocated_bytes,
            )?;
        }

        if !self.warnings.is_empty() {
            writeln!(f)?;
            writeln!(f, "Warnings:")?;
            for w in &self.warnings {
                writeln!(f, "  ⚠️  {}", w)?;
            }
        }

        Ok(())
    }
}

/// Measures memory bytes metered per contract invocation or execution step,
/// tracks host object allocations, and detects peak allocation spikes.
pub struct MemoryProfiler<'a> {
    env: &'a Env,
    warning_threshold_bytes: Option<u64>,
    steps: Vec<MemoryStep>,
    peak_memory_bytes: u64,
}

impl<'a> MemoryProfiler<'a> {
    /// Creates a new memory profiler bound to `env`.
    pub fn new(env: &'a Env) -> Self {
        #[allow(deprecated)]
        let initial_memory = env.budget().memory_bytes_cost();
        Self {
            env,
            warning_threshold_bytes: None,
            steps: Vec::new(),
            peak_memory_bytes: initial_memory,
        }
    }

    /// Sets a warning threshold in memory bytes (e.g. 500_000 for 500 KB).
    pub fn with_warning_threshold(mut self, threshold_bytes: u64) -> Self {
        self.warning_threshold_bytes = Some(threshold_bytes);
        self
    }

    /// Mutably updates the warning threshold.
    pub fn set_warning_threshold(&mut self, threshold_bytes: u64) -> &mut Self {
        self.warning_threshold_bytes = Some(threshold_bytes);
        self
    }

    /// Profiles a single closure or contract call labeled with `label`.
    ///
    /// Captures `env.budget().memory_bytes_cost()` before and after the closure,
    /// calculates allocation deltas, resets the cost tracker for intermediate host inspection,
    /// updates peak memory, and returns the closure's result.
    pub fn profile_call<T>(&mut self, label: impl Into<String>, call: impl FnOnce() -> T) -> T {
        let label_str = label.into();

        #[allow(deprecated)]
        let before_bytes = self.env.budget().memory_bytes_cost();
        self.env.cost_estimate().budget().reset_tracker();

        let result = call();

        #[allow(deprecated)]
        let after_bytes = self.env.budget().memory_bytes_cost();
        let budget = self.env.cost_estimate().budget();
        let alloc_tracker = budget.tracker(ContractCostType::MemAlloc);
        let copy_tracker = budget.tracker(ContractCostType::MemCpy);

        let delta = after_bytes.saturating_sub(before_bytes);
        if after_bytes > self.peak_memory_bytes {
            self.peak_memory_bytes = after_bytes;
        }

        let step = MemoryStep {
            label: label_str,
            memory_bytes_before: before_bytes,
            memory_bytes_after: after_bytes,
            memory_delta_bytes: delta,
            peak_live_bytes: after_bytes,
            host_allocations: alloc_tracker.iterations,
            host_allocated_bytes: alloc_tracker.inputs.unwrap_or(0),
            host_copied_bytes: copy_tracker.inputs.unwrap_or(0),
        };

        self.steps.push(step);
        result
    }

    /// Returns the highest memory bytes metered across all profiled steps.
    pub fn peak_memory_bytes(&self) -> u64 {
        self.peak_memory_bytes
    }

    /// Returns the recorded execution steps.
    pub fn steps(&self) -> &[MemoryStep] {
        &self.steps
    }

    /// Generates a structured [`MemoryProfileReport`] with metrics and warnings.
    pub fn report(&self) -> MemoryProfileReport {
        let mut total_delta = 0u64;
        let mut total_allocs = 0u64;
        let mut total_host_allocated_bytes = 0u64;
        let mut warnings = Vec::new();

        for step in &self.steps {
            total_delta = total_delta.saturating_add(step.memory_delta_bytes);
            total_allocs = total_allocs.saturating_add(step.host_allocations);
            total_host_allocated_bytes =
                total_host_allocated_bytes.saturating_add(step.host_allocated_bytes);

            if let Some(threshold) = self.warning_threshold_bytes {
                if step.memory_bytes_after >= threshold {
                    warnings.push(format!(
                        "Step '{}' reached {} memory bytes, exceeding threshold of {} bytes",
                        step.label, step.memory_bytes_after, threshold
                    ));
                }
            }
        }

        MemoryProfileReport {
            steps: self.steps.clone(),
            peak_memory_bytes: self.peak_memory_bytes,
            total_memory_delta: total_delta,
            total_host_allocations: total_allocs,
            total_host_allocated_bytes,
            warning_threshold_bytes: self.warning_threshold_bytes,
            warnings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env};

    #[contract]
    pub struct CollectionTester;

    #[contractimpl]
    impl CollectionTester {
        pub fn grow_vector(env: Env, count: u32) -> u32 {
            let mut v = soroban_sdk::Vec::new(&env);
            for i in 0..count {
                v.push_back(i);
            }
            v.len()
        }

        pub fn grow_map(env: Env, count: u32) -> u32 {
            let mut m = soroban_sdk::Map::new(&env);
            for i in 0..count {
                m.set(i, i * 10);
            }
            m.len()
        }
    }

    #[test]
    fn test_memory_profiler_vector_and_map_growth() {
        let env = Env::default();
        let id = env.register(CollectionTester, ());
        let client = CollectionTesterClient::new(&env, &id);

        let mut profiler = MemoryProfiler::new(&env).with_warning_threshold(100_000);

        profiler.profile_call("vector_100", || {
            client.grow_vector(&100);
        });

        profiler.profile_call("map_100", || {
            client.grow_map(&100);
        });

        assert_eq!(profiler.steps().len(), 2);
        assert_eq!(profiler.steps()[0].label, "vector_100");
        assert_eq!(profiler.steps()[1].label, "map_100");

        assert!(profiler.peak_memory_bytes() > 0);

        let report = profiler.report();
        assert_eq!(report.steps.len(), 2);

        // JSON report contains valid parseable keys
        let json = report.to_json();
        assert!(json.contains("peak_memory_bytes"));
        assert!(json.contains("vector_100"));
        assert!(json.contains("map_100"));

        // Text summary contains formatted header
        let text = format!("{}", report);
        assert!(text.contains("Soroban Memory Profile Report"));
        assert!(text.contains("vector_100"));
    }

    #[test]
    fn test_warning_threshold_triggers() {
        let env = Env::default();
        let id = env.register(CollectionTester, ());
        let client = CollectionTesterClient::new(&env, &id);

        let mut profiler = MemoryProfiler::new(&env).with_warning_threshold(1); // very low threshold

        profiler.profile_call("vector_50", || {
            client.grow_vector(&50);
        });

        let report = profiler.report();
        assert!(report.has_warnings());
        assert!(!report.warnings.is_empty());
        assert!(report.warnings[0].contains("exceeding threshold"));
    }
}
