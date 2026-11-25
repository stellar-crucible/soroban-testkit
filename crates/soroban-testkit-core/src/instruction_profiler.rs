//! Labeled CPU instruction measurements for test environments.

use soroban_sdk::Env;

use crate::budget::{BudgetRead, BudgetSnapshot};

/// CPU instructions consumed between two labeled checkpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionStep {
    /// Label for the code segment that ran before this checkpoint.
    pub label: String,
    /// CPU instructions attributed to this segment.
    pub cpu_insns: u64,
}

/// Samples cumulative host CPU metering at labeled test checkpoints.
///
/// Each checkpoint reports the instructions consumed since the previous
/// checkpoint (or construction). The profiler measures host-metered work; Rust
/// code executed outside a Soroban host call is not metered by the SDK.
pub struct InstructionProfiler {
    env: Env,
    previous_cpu_insns: u64,
    steps: Vec<InstructionStep>,
}

impl InstructionProfiler {
    /// Starts profiling `env` from its current cumulative CPU reading.
    pub fn new(env: &Env) -> Self {
        Self {
            env: env.clone(),
            previous_cpu_insns: cpu_insns(&env.cost_estimate().budget()),
            steps: Vec::new(),
        }
    }

    /// Records CPU instructions consumed since the previous checkpoint.
    pub fn checkpoint(&mut self, label: impl Into<String>) -> &mut Self {
        let current = cpu_insns(&self.env.cost_estimate().budget());
        self.record_checkpoint(label, current)
    }

    /// All recorded segments, in checkpoint order.
    pub fn steps(&self) -> &[InstructionStep] {
        &self.steps
    }

    /// Total CPU instructions covered by the recorded segments.
    pub fn total_cpu_insns(&self) -> u64 {
        self.steps
            .iter()
            .fold(0u64, |total, step| total.saturating_add(step.cpu_insns))
    }

    /// Segments consuming strictly more than `threshold_percent` of the total.
    ///
    /// The threshold is compared against all recorded segments and can be
    /// greater than 100 to intentionally return no hot paths.
    pub fn hot_paths_over(&self, threshold_percent: u8) -> Vec<&InstructionStep> {
        let total = u128::from(self.total_cpu_insns());
        if total == 0 {
            return Vec::new();
        }
        let threshold = u128::from(threshold_percent);

        self.steps
            .iter()
            .filter(|step| is_hot(step.cpu_insns, total, threshold))
            .collect()
    }

    /// Formats a text table, marking segments over the supplied share threshold.
    pub fn format_text(&self, threshold_percent: u8) -> String {
        self.format_table(threshold_percent, false)
    }

    /// Formats a Markdown table, marking segments over the supplied share threshold.
    pub fn format_markdown(&self, threshold_percent: u8) -> String {
        self.format_table(threshold_percent, true)
    }

    fn record_checkpoint(&mut self, label: impl Into<String>, current_cpu_insns: u64) -> &mut Self {
        let cpu_insns = current_cpu_insns.saturating_sub(self.previous_cpu_insns);
        self.previous_cpu_insns = current_cpu_insns;
        self.steps.push(InstructionStep {
            label: label.into(),
            cpu_insns,
        });
        self
    }

    fn format_table(&self, threshold_percent: u8, markdown: bool) -> String {
        let total = self.total_cpu_insns();
        let mut output = if markdown {
            String::from("| Segment | CPU instructions | Share | |\n|---|---:|---:|:---|\n")
        } else {
            String::from("Segment                 CPU instructions    Share  Flag\n")
        };
        let threshold = u128::from(threshold_percent);

        for step in &self.steps {
            let share = if total == 0 {
                0.0
            } else {
                step.cpu_insns as f64 * 100.0 / total as f64
            };
            let hot = is_hot(step.cpu_insns, u128::from(total), threshold);
            if markdown {
                let label = if hot {
                    format!("**{}**", step.label)
                } else {
                    step.label.clone()
                };
                let flag = if hot { "HOT" } else { "" };
                output.push_str(&format!(
                    "| {label} | {} | {share:.1}% | {flag} |\n",
                    step.cpu_insns
                ));
            } else {
                output.push_str(&format!(
                    "{:<24} {:>16} {:>7.1}%  {}\n",
                    step.label,
                    step.cpu_insns,
                    share,
                    if hot { "HOT" } else { "" }
                ));
            }
        }

        output.push_str(&if markdown {
            format!(
                "| **Total** | **{total}** | **{:.1}%** | |\n",
                if total == 0 { 0.0 } else { 100.0 }
            )
        } else {
            format!(
                "{:<24} {:>16} {:>7.1}%\n",
                "Total",
                total,
                if total == 0 { 0.0 } else { 100.0 }
            )
        });
        output
    }
}

fn is_hot(cpu_insns: u64, total: u128, threshold: u128) -> bool {
    total > 0 && u128::from(cpu_insns) * 100 > total * threshold
}

fn cpu_insns(budget: &impl BudgetRead) -> u64 {
    BudgetSnapshot::capture(budget).cpu_insns
}

#[cfg(test)]
mod tests {
    use super::InstructionProfiler;
    use crate::budget::BudgetSnapshot;
    use soroban_env_host::{budget::Budget, xdr::ContractCostType};
    use soroban_sdk::{contract, contractimpl, symbol_short, Env};

    #[test]
    fn measures_deltas_between_labeled_checkpoints() {
        let env = Env::default();
        let budget = Budget::default();
        let mut profiler = InstructionProfiler::new(&env);
        let mut readings = Vec::new();

        for (label, iterations) in [("small loop", 5u64), ("large loop", 20)] {
            let simulated_math_cost = (0..iterations)
                .map(|value| value.saturating_mul(value).saturating_add(1))
                .sum::<u64>();
            for _ in 0..simulated_math_cost {
                budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
            }
            let reading = BudgetSnapshot::capture(&budget).cpu_insns;
            readings.push(reading);
            profiler.record_checkpoint(label, reading);
        }

        assert_eq!(profiler.steps()[0].label, "small loop");
        assert_eq!(profiler.steps()[0].cpu_insns, readings[0]);
        assert_eq!(profiler.steps()[1].label, "large loop");
        assert_eq!(profiler.steps()[1].cpu_insns, readings[1] - readings[0]);
        assert_eq!(profiler.total_cpu_insns(), readings[1]);
    }

    #[contract]
    struct Meter;

    #[contractimpl]
    impl Meter {
        pub fn bump(env: Env) {
            env.storage().instance().set(&symbol_short!("n"), &1u32);
        }
    }

    #[test]
    fn samples_metered_work_from_the_environment() {
        let env = Env::default();
        let mut profiler = InstructionProfiler::new(&env);
        let contract = env.register(Meter, ());
        let client = MeterClient::new(&env, &contract);

        client.bump();
        profiler.checkpoint("write");

        assert!(profiler.steps()[0].cpu_insns > 0);
    }

    #[test]
    fn formats_text_and_markdown_and_flags_hot_segments() {
        let env = Env::default();
        let mut profiler = InstructionProfiler::new(&env);
        profiler.record_checkpoint("small", 10);
        profiler.record_checkpoint("hot path", 100);

        let hot_paths = profiler.hot_paths_over(50);
        assert_eq!(hot_paths.len(), 1);
        assert_eq!(hot_paths[0].label, "hot path");

        let text = profiler.format_text(50);
        assert!(text.contains("hot path"));
        assert!(text.contains("HOT"));
        let markdown = profiler.format_markdown(50);
        assert!(markdown.contains("**hot path**"));
        assert!(markdown.contains("| **Total** | **100**"));
    }

    #[test]
    fn zero_cost_and_threshold_edges_are_handled() {
        let env = Env::default();
        let mut empty = InstructionProfiler::new(&env);
        empty.record_checkpoint("idle", 0);
        assert!(empty.hot_paths_over(0).is_empty());

        let mut measured = InstructionProfiler::new(&env);
        measured.record_checkpoint("all work", 100);
        assert_eq!(measured.hot_paths_over(100).len(), 0);
        assert_eq!(measured.hot_paths_over(99).len(), 1);
    }
}
