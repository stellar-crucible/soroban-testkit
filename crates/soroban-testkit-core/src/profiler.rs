use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExecutionMetrics {
    pub map_lookups: u64,
    pub objects_created: u64,
    pub memory_copied_bytes: u64,
}

impl HostExecutionMetrics {
    pub fn record_map_lookup(&mut self) {
        self.map_lookups = self.map_lookups.saturating_add(1);
    }

    pub fn record_object_created(&mut self) {
        self.objects_created = self.objects_created.saturating_add(1);
    }

    pub fn record_memory_copy(&mut self, bytes: usize) {
        self.memory_copied_bytes = self
            .memory_copied_bytes
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostExecutionSummary {
    pub iterations: usize,
    pub mean_latency_ns: f64,
    pub p50_latency_ns: u64,
    pub p95_latency_ns: u64,
    pub p99_latency_ns: u64,
    pub mean_map_lookups: f64,
    pub mean_objects_created: f64,
    pub mean_memory_copied_bytes: f64,
    pub samples: Vec<HostExecutionSample>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostExecutionSample {
    pub elapsed_ns: u64,
    pub metrics: HostExecutionMetrics,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfilerError {
    ZeroIterations,
}

impl std::fmt::Display for ProfilerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroIterations => formatter.write_str("profiler requires at least one iteration"),
        }
    }
}

impl std::error::Error for ProfilerError {}

/// Repeatedly runs a contract invocation and summarizes wall-clock latency.
///
/// The Soroban host does not expose per-invocation map/object counters through
/// its stable public API. The invocation closure therefore records those
/// operations explicitly in the supplied metrics value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostExecutionProfiler {
    iterations: usize,
}

impl HostExecutionProfiler {
    pub fn new(iterations: usize) -> Self {
        Self { iterations }
    }

    pub fn run(
        &self,
        mut invocation: impl FnMut(&mut HostExecutionMetrics),
    ) -> Result<HostExecutionSummary, ProfilerError> {
        if self.iterations == 0 {
            return Err(ProfilerError::ZeroIterations);
        }

        let mut samples = Vec::with_capacity(self.iterations);
        for _ in 0..self.iterations {
            let mut metrics = HostExecutionMetrics::default();
            let start = Instant::now();
            invocation(&mut metrics);
            let elapsed_ns = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
            samples.push(HostExecutionSample {
                elapsed_ns,
                metrics,
            });
        }

        let mut latencies: Vec<u64> = samples.iter().map(|sample| sample.elapsed_ns).collect();
        latencies.sort_unstable();
        let count = self.iterations as f64;
        let sum = |select: fn(&HostExecutionMetrics) -> u64| {
            samples.iter().fold(0u64, |total, sample| {
                total.saturating_add(select(&sample.metrics))
            }) as f64
                / count
        };

        Ok(HostExecutionSummary {
            iterations: self.iterations,
            mean_latency_ns: latencies.iter().map(|value| *value as f64).sum::<f64>() / count,
            p50_latency_ns: percentile(&latencies, 50),
            p95_latency_ns: percentile(&latencies, 95),
            p99_latency_ns: percentile(&latencies, 99),
            mean_map_lookups: sum(|metrics| metrics.map_lookups),
            mean_objects_created: sum(|metrics| metrics.objects_created),
            mean_memory_copied_bytes: sum(|metrics| metrics.memory_copied_bytes),
            samples,
        })
    }
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let rank = (sorted.len() * percentile).div_ceil(100).max(1);
    sorted[rank - 1]
}

impl HostExecutionSummary {
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn to_markdown(&self) -> String {
        format!(
            "| Metric | Result |\n| --- | ---: |\n| Iterations | {} |\n| Mean latency (ns) | {:.2} |\n| p50 latency (ns) | {} |\n| p95 latency (ns) | {} |\n| p99 latency (ns) | {} |\n| Mean map lookups / invocation | {:.2} |\n| Mean objects created / invocation | {:.2} |\n| Mean memory copied (bytes / invocation) | {:.2} |",
            self.iterations,
            self.mean_latency_ns,
            self.p50_latency_ns,
            self.p95_latency_ns,
            self.p99_latency_ns,
            self.mean_map_lookups,
            self.mean_objects_created,
            self.mean_memory_copied_bytes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{HostExecutionMetrics, HostExecutionProfiler, ProfilerError};

    #[test]
    fn records_operation_counts_per_invocation_and_exports_both_formats() {
        let summary = HostExecutionProfiler::new(4)
            .run(|metrics| {
                metrics.record_map_lookup();
                metrics.record_map_lookup();
                metrics.record_object_created();
                metrics.record_memory_copy(16);
            })
            .unwrap();

        assert_eq!(summary.iterations, 4);
        assert_eq!(summary.mean_map_lookups, 2.0);
        assert_eq!(summary.mean_objects_created, 1.0);
        assert_eq!(summary.mean_memory_copied_bytes, 16.0);
        assert!(summary.p50_latency_ns <= summary.p95_latency_ns);
        assert!(summary.p95_latency_ns <= summary.p99_latency_ns);
        assert!(summary.to_markdown().contains("| p99 latency (ns) |"));
        assert!(summary.to_json().unwrap().contains("\"samples\""));
    }

    #[test]
    fn percentile_rank_uses_nearest_rank_for_small_samples() {
        let summary = HostExecutionProfiler::new(1).run(|_| {}).unwrap();
        assert_eq!(summary.p50_latency_ns, summary.p99_latency_ns);
    }

    #[test]
    fn zero_iterations_returns_a_clear_error() {
        assert_eq!(
            HostExecutionProfiler::new(0).run(|_| {}).unwrap_err(),
            ProfilerError::ZeroIterations
        );
    }

    #[test]
    fn metrics_saturate_instead_of_wrapping() {
        let mut metrics = HostExecutionMetrics {
            map_lookups: u64::MAX,
            objects_created: u64::MAX,
            memory_copied_bytes: u64::MAX,
        };

        metrics.record_map_lookup();
        metrics.record_object_created();
        metrics.record_memory_copy(1);

        assert_eq!(metrics.map_lookups, u64::MAX);
        assert_eq!(metrics.objects_created, u64::MAX);
        assert_eq!(metrics.memory_copied_bytes, u64::MAX);
    }
}
