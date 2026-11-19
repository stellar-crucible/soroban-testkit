//! Budget differential and relative gas savings assertion tooling.
//!
//! When refactoring code paths or evaluating alternative algorithm implementations
//! (e.g. linear scan vs binary search, or unindexed vs indexed lookups), developers
//! must ensure that optimized implementations achieve target relative gas savings
//! and do not regress in CPU or memory consumption.
//!
//! [`BudgetDiffMatcher`] and the [`assert_budget_diff_within!`] macro execute baseline
//! and optimized test closures, measure their CPU instruction and memory byte consumption,
//! assert relative savings targets, and format an informative side-by-side comparative
//! diff table whenever an assertion fails.

use std::fmt;
use soroban_sdk::Env;
use soroban_testkit_core::budget::BudgetSnapshot;

/// One rejection reason encountered during budget comparison.
#[derive(Debug, Clone, PartialEq)]
pub enum BudgetDiffViolation {
    /// Optimized path consumed more CPU instructions than baseline (regression).
    CpuExceededBaseline {
        baseline: u64,
        optimized: u64,
        regression_percent: f64,
    },
    /// Optimized path consumed more memory bytes than baseline (regression).
    MemExceededBaseline {
        baseline: u64,
        optimized: u64,
        regression_percent: f64,
    },
    /// Optimized path failed to achieve the target CPU savings percentage.
    CpuTargetSavingsUnmet {
        achieved_percent: f64,
        target_percent: f64,
    },
    /// Optimized path failed to achieve the target memory savings percentage.
    MemTargetSavingsUnmet {
        achieved_percent: f64,
        target_percent: f64,
    },
    /// Optimized path failed to achieve the minimum absolute CPU instruction savings.
    CpuAbsoluteSavingsUnmet {
        achieved: i64,
        required: u64,
    },
    /// Optimized path failed to achieve the minimum absolute memory byte savings.
    MemAbsoluteSavingsUnmet {
        achieved: i64,
        required: u64,
    },
    /// No savings were achieved across either CPU or memory.
    NoSavingsAchieved,
}

impl fmt::Display for BudgetDiffViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CpuExceededBaseline {
                baseline,
                optimized,
                regression_percent,
            } => write!(
                f,
                "Optimized CPU exceeded baseline: {optimized} > {baseline} (+{reg} insns, {regression_percent:+.2}% regression)",
                reg = format_commas(optimized.saturating_sub(*baseline))
            ),
            Self::MemExceededBaseline {
                baseline,
                optimized,
                regression_percent,
            } => write!(
                f,
                "Optimized memory exceeded baseline: {optimized} > {baseline} (+{reg} bytes, {regression_percent:+.2}% regression)",
                reg = format_commas(optimized.saturating_sub(*baseline))
            ),
            Self::CpuTargetSavingsUnmet {
                achieved_percent,
                target_percent,
            } => write!(
                f,
                "Failed to achieve target CPU savings: achieved {achieved_percent:.2}%, expected at least {target_percent:.2}%"
            ),
            Self::MemTargetSavingsUnmet {
                achieved_percent,
                target_percent,
            } => write!(
                f,
                "Failed to achieve target memory savings: achieved {achieved_percent:.2}%, expected at least {target_percent:.2}%"
            ),
            Self::CpuAbsoluteSavingsUnmet { achieved, required } => write!(
                f,
                "Failed to achieve minimum CPU savings: achieved {achieved} insns, required at least {required} insns"
            ),
            Self::MemAbsoluteSavingsUnmet { achieved, required } => write!(
                f,
                "Failed to achieve minimum memory savings: achieved {achieved} bytes, required at least {required} bytes"
            ),
            Self::NoSavingsAchieved => write!(
                f,
                "Optimized path failed to achieve any gas savings over baseline (0.00% CPU and memory reduction)"
            ),
        }
    }
}

/// Helper to format an unsigned integer with comma thousand separators.
pub fn format_commas(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let rem = s.len() % 3;
    let initial = if rem == 0 { 3 } else { rem };

    out.push_str(&s[..initial]);
    for chunk in s[initial..].as_bytes().chunks(3) {
        out.push(',');
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
    }
    out
}

/// Helper to format a signed integer with a leading sign and comma thousand separators.
pub fn format_signed_commas(n: i64) -> String {
    if n < 0 {
        format!("-{}", format_commas(n.unsigned_abs()))
    } else if n > 0 {
        format!("+{}", format_commas(n as u64))
    } else {
        "0".to_string()
    }
}

/// Calculates relative percentage savings: `((baseline - optimized) / baseline) * 100.0`.
///
/// Positive values indicate gas savings/reduction; negative values indicate regression.
pub fn calculate_savings_percent(baseline: u64, optimized: u64) -> f64 {
    if baseline == 0 {
        if optimized == 0 {
            0.0
        } else {
            -100.0
        }
    } else {
        ((baseline as f64 - optimized as f64) / baseline as f64) * 100.0
    }
}

/// Detailed comparative report evaluating baseline versus optimized execution.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetDiffReport {
    /// Baseline consumption snapshot.
    pub baseline: BudgetSnapshot,
    /// Optimized branch consumption snapshot.
    pub optimized: BudgetSnapshot,
    /// Signed CPU difference (`optimized - baseline`).
    pub cpu_diff: i64,
    /// Signed memory difference (`optimized - baseline`).
    pub mem_diff: i64,
    /// Absolute CPU savings (`baseline - optimized`).
    pub cpu_savings: i64,
    /// Absolute memory savings (`baseline - optimized`).
    pub mem_savings: i64,
    /// Relative CPU savings percentage.
    pub cpu_savings_percent: f64,
    /// Relative memory savings percentage.
    pub mem_savings_percent: f64,
    /// Required minimum CPU savings percentage.
    pub min_cpu_savings_percent: Option<f64>,
    /// Required minimum memory savings percentage.
    pub min_mem_savings_percent: Option<f64>,
    /// Required minimum absolute CPU instruction savings.
    pub min_cpu_savings: Option<u64>,
    /// Required minimum absolute memory byte savings.
    pub min_mem_savings: Option<u64>,
    /// Whether CPU regressions are permitted.
    pub allow_cpu_regression: bool,
    /// Whether memory regressions are permitted.
    pub allow_mem_regression: bool,
    /// List of violations encountered.
    pub violations: Vec<BudgetDiffViolation>,
    /// Whether CPU evaluation passed.
    pub cpu_passed: bool,
    /// Whether memory evaluation passed.
    pub mem_passed: bool,
}

impl BudgetDiffReport {
    /// Returns true if all savings targets and regression constraints are satisfied.
    pub fn is_satisfied(&self) -> bool {
        self.violations.is_empty()
    }

    /// Asserts that the report is satisfied, panicking with a formatted diff table if breached.
    pub fn assert_satisfied(&self) {
        if !self.is_satisfied() {
            panic!("\n{}\n", self.format_table());
        }
    }

    /// Formats a side-by-side consumption table with clear metrics, deltas, and violation summary.
    pub fn format_table(&self) -> String {
        let mut out = String::new();

        out.push_str("Budget Differential Assertion Failed!\n");
        out.push_str("Optimized branch failed to achieve required gas savings or exceeded baseline limits.\n\n");

        let col_metric_w = 18;
        let col_base_w = 16;
        let col_opt_w = 16;
        let col_diff_w = 18;
        let col_savings_w = 20;
        let col_target_w = 16;
        let col_status_w = 8;

        let total_w = col_metric_w
            + col_base_w
            + col_opt_w
            + col_diff_w
            + col_savings_w
            + col_target_w
            + col_status_w
            + 22;

        let divider = "=".repeat(total_w);
        let inner_divider = "-".repeat(total_w);

        out.push_str(&divider);
        out.push('\n');
        use std::fmt::Write as _;
        let _ = writeln!(
            out,
            "| {:<w_m$} | {:>w_b$} | {:>w_o$} | {:>w_d$} | {:>w_s$} | {:<w_t$} | {:^w_st$} |",
            "Metric",
            "Baseline",
            "Optimized",
            "Delta (Opt-Base)",
            "Savings (%)",
            "Target",
            "Status",
            w_m = col_metric_w,
            w_b = col_base_w,
            w_o = col_opt_w,
            w_d = col_diff_w,
            w_s = col_savings_w,
            w_t = col_target_w,
            w_st = col_status_w,
        );
        out.push_str(&divider);
        out.push('\n');

        // CPU row
        let cpu_target_str = if let Some(target) = self.min_cpu_savings_percent {
            format!(">= {:.2}%", target)
        } else if let Some(abs) = self.min_cpu_savings {
            format!(">= {} insns", format_commas(abs))
        } else if self.allow_cpu_regression {
            "unconstrained".to_string()
        } else {
            "< baseline".to_string()
        };

        let cpu_savings_str = if self.cpu_savings_percent >= 0.0 {
            format!("+{:.2}% savings", self.cpu_savings_percent)
        } else {
            format!("{:.2}% (REGRESS)", self.cpu_savings_percent)
        };

        let _ = writeln!(
            out,
            "| {:<w_m$} | {:>w_b$} | {:>w_o$} | {:>w_d$} | {:>w_s$} | {:<w_t$} | {:^w_st$} |",
            "CPU Instructions",
            format_commas(self.baseline.cpu_insns),
            format_commas(self.optimized.cpu_insns),
            format_signed_commas(self.cpu_diff),
            cpu_savings_str,
            cpu_target_str,
            if self.cpu_passed { "PASS" } else { "FAIL" },
            w_m = col_metric_w,
            w_b = col_base_w,
            w_o = col_opt_w,
            w_d = col_diff_w,
            w_s = col_savings_w,
            w_t = col_target_w,
            w_st = col_status_w,
        );

        out.push_str(&inner_divider);
        out.push('\n');

        // Memory row
        let mem_target_str = if let Some(target) = self.min_mem_savings_percent {
            format!(">= {:.2}%", target)
        } else if let Some(abs) = self.min_mem_savings {
            format!(">= {} B", format_commas(abs))
        } else if self.allow_mem_regression {
            "unconstrained".to_string()
        } else {
            "<= baseline".to_string()
        };

        let mem_savings_str = if self.mem_savings_percent >= 0.0 {
            format!("+{:.2}% savings", self.mem_savings_percent)
        } else {
            format!("{:.2}% (REGRESS)", self.mem_savings_percent)
        };

        let _ = writeln!(
            out,
            "| {:<w_m$} | {:>w_b$} | {:>w_o$} | {:>w_d$} | {:>w_s$} | {:<w_t$} | {:^w_st$} |",
            "Memory (Bytes)",
            format_commas(self.baseline.mem_bytes),
            format_commas(self.optimized.mem_bytes),
            format_signed_commas(self.mem_diff),
            mem_savings_str,
            mem_target_str,
            if self.mem_passed { "PASS" } else { "FAIL" },
            w_m = col_metric_w,
            w_b = col_base_w,
            w_o = col_opt_w,
            w_d = col_diff_w,
            w_s = col_savings_w,
            w_t = col_target_w,
            w_st = col_status_w,
        );

        out.push_str(&divider);
        out.push('\n');

        if !self.violations.is_empty() {
            out.push_str("\nFailure Summary:\n");
            for v in &self.violations {
                let _ = writeln!(out, "  • {v}");
            }
        }

        out
    }
}

/// Measures the budget consumed by executing a closure within a Soroban environment.
pub fn measure_invocation_budget<R>(
    env: &Env,
    invocation: impl FnOnce() -> R,
) -> (R, BudgetSnapshot) {
    let budget = env.cost_estimate().budget();
    let before_budget = BudgetSnapshot {
        cpu_insns: budget.cpu_instruction_cost(),
        mem_bytes: budget.memory_bytes_cost(),
    };

    let result = invocation();

    let budget = env.cost_estimate().budget();
    let after_budget = BudgetSnapshot {
        cpu_insns: budget.cpu_instruction_cost(),
        mem_bytes: budget.memory_bytes_cost(),
    };

    let budget_delta = BudgetSnapshot {
        cpu_insns: after_budget.cpu_insns.saturating_sub(before_budget.cpu_insns),
        mem_bytes: after_budget.mem_bytes.saturating_sub(before_budget.mem_bytes),
    };

    let consumed = if budget_delta.cpu_insns > 0 || budget_delta.mem_bytes > 0 {
        budget_delta
    } else {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            BudgetSnapshot::last_invocation(env)
        }))
        .unwrap_or(budget_delta)
    };

    (result, consumed)
}

/// Builder configuring budget differential and relative savings assertions.
#[derive(Debug, Clone, Default)]
pub struct BudgetDiffMatcher {
    min_cpu_savings_percent: Option<f64>,
    min_mem_savings_percent: Option<f64>,
    min_cpu_savings: Option<u64>,
    min_mem_savings: Option<u64>,
    allow_cpu_regression: bool,
    allow_mem_regression: bool,
}

impl BudgetDiffMatcher {
    /// Creates a new unconstrained matcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Require minimum relative CPU savings percentage (e.g. `20.0` for >= 20% reduction).
    pub fn min_cpu_savings_percent(mut self, percent: f64) -> Self {
        self.min_cpu_savings_percent = Some(percent);
        self
    }

    /// Require minimum relative memory savings percentage (e.g. `10.0` for >= 10% reduction).
    pub fn min_mem_savings_percent(mut self, percent: f64) -> Self {
        self.min_mem_savings_percent = Some(percent);
        self
    }

    /// Require minimum relative savings percentage applied to CPU instructions.
    pub fn min_savings_percent(mut self, percent: f64) -> Self {
        self.min_cpu_savings_percent = Some(percent);
        self
    }

    /// Alias for [`Self::min_savings_percent`].
    pub fn target_savings_percent(self, percent: f64) -> Self {
        self.min_savings_percent(percent)
    }

    /// Require minimum absolute CPU instruction savings.
    pub fn min_cpu_savings(mut self, instructions: u64) -> Self {
        self.min_cpu_savings = Some(instructions);
        self
    }

    /// Require minimum absolute memory byte savings.
    pub fn min_mem_savings(mut self, bytes: u64) -> Self {
        self.min_mem_savings = Some(bytes);
        self
    }

    /// Permit CPU consumption regressions in the optimized path.
    pub fn allow_cpu_regression(mut self, allow: bool) -> Self {
        self.allow_cpu_regression = allow;
        self
    }

    /// Permit memory consumption regressions in the optimized path.
    pub fn allow_mem_regression(mut self, allow: bool) -> Self {
        self.allow_mem_regression = allow;
        self
    }

    /// Evaluates two captured budget snapshots and generates a comparative report.
    pub fn compare_snapshots(
        &self,
        baseline: BudgetSnapshot,
        optimized: BudgetSnapshot,
    ) -> BudgetDiffReport {
        let cpu_diff = optimized.cpu_insns as i64 - baseline.cpu_insns as i64;
        let mem_diff = optimized.mem_bytes as i64 - baseline.mem_bytes as i64;
        let cpu_savings = baseline.cpu_insns as i64 - optimized.cpu_insns as i64;
        let mem_savings = baseline.mem_bytes as i64 - optimized.mem_bytes as i64;

        let cpu_savings_percent = calculate_savings_percent(baseline.cpu_insns, optimized.cpu_insns);
        let mem_savings_percent = calculate_savings_percent(baseline.mem_bytes, optimized.mem_bytes);

        let mut violations = Vec::new();
        let mut cpu_passed = true;
        let mut mem_passed = true;

        // Check CPU regression
        if optimized.cpu_insns > baseline.cpu_insns && !self.allow_cpu_regression {
            let reg_pct = calculate_savings_percent(baseline.cpu_insns, optimized.cpu_insns);
            violations.push(BudgetDiffViolation::CpuExceededBaseline {
                baseline: baseline.cpu_insns,
                optimized: optimized.cpu_insns,
                regression_percent: reg_pct,
            });
            cpu_passed = false;
        }

        // Check CPU target savings
        if let Some(target) = self.min_cpu_savings_percent {
            if cpu_savings_percent < target {
                violations.push(BudgetDiffViolation::CpuTargetSavingsUnmet {
                    achieved_percent: cpu_savings_percent,
                    target_percent: target,
                });
                cpu_passed = false;
            }
        }

        // Check CPU absolute savings
        if let Some(min_abs) = self.min_cpu_savings {
            if cpu_savings < min_abs as i64 {
                violations.push(BudgetDiffViolation::CpuAbsoluteSavingsUnmet {
                    achieved: cpu_savings,
                    required: min_abs,
                });
                cpu_passed = false;
            }
        }

        // Check Memory regression
        if optimized.mem_bytes > baseline.mem_bytes && !self.allow_mem_regression {
            let reg_pct = calculate_savings_percent(baseline.mem_bytes, optimized.mem_bytes);
            violations.push(BudgetDiffViolation::MemExceededBaseline {
                baseline: baseline.mem_bytes,
                optimized: optimized.mem_bytes,
                regression_percent: reg_pct,
            });
            mem_passed = false;
        }

        // Check Memory target savings
        if let Some(target) = self.min_mem_savings_percent {
            if mem_savings_percent < target {
                violations.push(BudgetDiffViolation::MemTargetSavingsUnmet {
                    achieved_percent: mem_savings_percent,
                    target_percent: target,
                });
                mem_passed = false;
            }
        }

        // Check Memory absolute savings
        if let Some(min_abs) = self.min_mem_savings {
            if mem_savings < min_abs as i64 {
                violations.push(BudgetDiffViolation::MemAbsoluteSavingsUnmet {
                    achieved: mem_savings,
                    required: min_abs,
                });
                mem_passed = false;
            }
        }

        // Default comparison mode: when no explicit targets or exemptions are configured,
        // optimized branch MUST achieve strictly lower consumption in at least one metric
        // without regressing on either metric.
        let has_explicit_targets = self.min_cpu_savings_percent.is_some()
            || self.min_mem_savings_percent.is_some()
            || self.min_cpu_savings.is_some()
            || self.min_mem_savings.is_some();

        if !has_explicit_targets
            && !self.allow_cpu_regression
            && !self.allow_mem_regression
            && optimized.cpu_insns == baseline.cpu_insns
            && optimized.mem_bytes == baseline.mem_bytes
        {
            violations.push(BudgetDiffViolation::NoSavingsAchieved);
            cpu_passed = false;
            mem_passed = false;
        }

        BudgetDiffReport {
            baseline,
            optimized,
            cpu_diff,
            mem_diff,
            cpu_savings,
            mem_savings,
            cpu_savings_percent,
            mem_savings_percent,
            min_cpu_savings_percent: self.min_cpu_savings_percent,
            min_mem_savings_percent: self.min_mem_savings_percent,
            min_cpu_savings: self.min_cpu_savings,
            min_mem_savings: self.min_mem_savings,
            allow_cpu_regression: self.allow_cpu_regression,
            allow_mem_regression: self.allow_mem_regression,
            violations,
            cpu_passed,
            mem_passed,
        }
    }

    /// Measures and compares two test closures in a shared environment.
    pub fn compare<R1, R2>(
        &self,
        env: &Env,
        baseline: impl FnOnce() -> R1,
        optimized: impl FnOnce() -> R2,
    ) -> ((R1, R2), BudgetDiffReport) {
        let (base_res, base_budget) = measure_invocation_budget(env, baseline);
        let (opt_res, opt_budget) = measure_invocation_budget(env, optimized);

        let report = self.compare_snapshots(base_budget, opt_budget);
        ((base_res, opt_res), report)
    }

    /// Measures and compares two test closures across separate baseline and optimized environments.
    pub fn compare_dual<R1, R2>(
        &self,
        baseline_env: &Env,
        baseline: impl FnOnce() -> R1,
        optimized_env: &Env,
        optimized: impl FnOnce() -> R2,
    ) -> ((R1, R2), BudgetDiffReport) {
        let (base_res, base_budget) = measure_invocation_budget(baseline_env, baseline);
        let (opt_res, opt_budget) = measure_invocation_budget(optimized_env, optimized);

        let report = self.compare_snapshots(base_budget, opt_budget);
        ((base_res, opt_res), report)
    }

    /// Executes both closures and asserts satisfaction in a single environment.
    pub fn assert_diff<R1, R2>(
        &self,
        env: &Env,
        baseline: impl FnOnce() -> R1,
        optimized: impl FnOnce() -> R2,
    ) -> (R1, R2) {
        let (results, report) = self.compare(env, baseline, optimized);
        report.assert_satisfied();
        results
    }

    /// Executes both closures and asserts satisfaction across separate environments.
    pub fn assert_diff_dual<R1, R2>(
        &self,
        baseline_env: &Env,
        baseline: impl FnOnce() -> R1,
        optimized_env: &Env,
        optimized: impl FnOnce() -> R2,
    ) -> (R1, R2) {
        let (results, report) = self.compare_dual(baseline_env, baseline, optimized_env, optimized);
        report.assert_satisfied();
        results
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __apply_budget_diff_option {
    ($matcher:expr, min_cpu_savings_percent, $val:expr) => {
        $matcher.min_cpu_savings_percent($val as f64)
    };
    ($matcher:expr, min_mem_savings_percent, $val:expr) => {
        $matcher.min_mem_savings_percent($val as f64)
    };
    ($matcher:expr, min_savings_percent, $val:expr) => {
        $matcher.min_savings_percent($val as f64)
    };
    ($matcher:expr, target_savings_percent, $val:expr) => {
        $matcher.target_savings_percent($val as f64)
    };
    ($matcher:expr, min_cpu_savings, $val:expr) => {
        $matcher.min_cpu_savings($val as u64)
    };
    ($matcher:expr, min_mem_savings, $val:expr) => {
        $matcher.min_mem_savings($val as u64)
    };
    ($matcher:expr, allow_cpu_regression, $val:expr) => {
        $matcher.allow_cpu_regression($val)
    };
    ($matcher:expr, allow_mem_regression, $val:expr) => {
        $matcher.allow_mem_regression($val)
    };
}

/// Assert that an optimized code path consumes less CPU or memory than a baseline branch,
/// validating relative gas savings and formatting a comparative diff table on failure.
///
/// # Supported Syntaxes
///
/// ### Standard Single Environment
/// ```rust,ignore
/// assert_budget_diff_within!(
///     env,
///     baseline: {
///         linear_search(&client, &target);
///     },
///     optimized: {
///         binary_search(&client, &target);
///     },
///     min_cpu_savings_percent: 20.0,
/// );
/// ```
///
/// ### With Closures
/// ```rust,ignore
/// assert_budget_diff_within!(
///     &env,
///     baseline: || client.linear(),
///     optimized: || client.binary(),
///     min_savings_percent: 15.0,
/// );
/// ```
///
/// ### Dual Environments (for state-modifying operations)
/// ```rust,ignore
/// assert_budget_diff_within!(
///     baseline: (&env1, { client1.sort_bubble(); }),
///     optimized: (&env2, { client2.sort_quick(); }),
///     min_cpu_savings_percent: 30.0,
/// );
/// ```
#[macro_export]
macro_rules! assert_budget_diff_within {
    // 1. Dual env syntax with tuple: baseline: (&env1, ...), optimized: (&env2, ...)
    (
        baseline: ($env_base:expr, $base_block:block),
        optimized: ($env_opt:expr, $opt_block:block)
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff_dual($env_base, || $base_block, $env_opt, || $opt_block)
    }};

    (
        baseline: ($env_base:expr, $base_fn:expr),
        optimized: ($env_opt:expr, $opt_fn:expr)
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff_dual($env_base, $base_fn, $env_opt, $opt_fn)
    }};

    // 2. Dual env syntax with explicit baseline_env and optimized_env:
    (
        baseline_env: $env_base:expr,
        optimized_env: $env_opt:expr,
        baseline: $base_block:block,
        optimized: $opt_block:block
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff_dual($env_base, || $base_block, $env_opt, || $opt_block)
    }};

    (
        baseline_env: $env_base:expr,
        optimized_env: $env_opt:expr,
        baseline: $base_fn:expr,
        optimized: $opt_fn:expr
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff_dual($env_base, $base_fn, $env_opt, $opt_fn)
    }};

    // 3. Single env syntax with named env: &env
    (
        env: $env:expr,
        baseline: $base_block:block,
        optimized: $opt_block:block
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff($env, || $base_block, || $opt_block)
    }};

    (
        env: $env:expr,
        baseline: $base_fn:expr,
        optimized: $opt_fn:expr
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff($env, $base_fn, $opt_fn)
    }};

    // 4. Positional env as first argument: assert_budget_diff_within!(env, baseline: { ... }, optimized: { ... }, ...)
    (
        $env:expr,
        baseline: $base_block:block,
        optimized: $opt_block:block
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff($env, || $base_block, || $opt_block)
    }};

    (
        $env:expr,
        baseline: $base_fn:expr,
        optimized: $opt_fn:expr
        $(, $opt_name:ident : $opt_val:expr)* $(,)?
    ) => {{
        #[allow(unused_mut)]
        let mut matcher = $crate::budget_diff::BudgetDiffMatcher::new();
        $(
            matcher = $crate::__apply_budget_diff_option!(matcher, $opt_name, $opt_val);
        )*
        matcher.assert_diff($env, $base_fn, $opt_fn)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, vec, Env, Vec};

    #[contract]
    pub struct SearchContract;

    #[contractimpl]
    impl SearchContract {
        /// Baseline: O(N) linear scan over a vector of values.
        pub fn linear_search(_env: Env, items: Vec<u64>, target: u64) -> Option<u32> {
            for i in 0..items.len() {
                if items.get(i).unwrap() == target {
                    return Some(i);
                }
            }
            None
        }

        /// Optimized: O(log N) binary search over a sorted vector of values.
        pub fn binary_search(_env: Env, items: Vec<u64>, target: u64) -> Option<u32> {
            if items.is_empty() {
                return None;
            }
            let mut low = 0u32;
            let mut high = items.len() - 1;

            while low <= high {
                let mid = low + (high - low) / 2;
                let val = items.get(mid).unwrap();
                if val == target {
                    return Some(mid);
                } else if val < target {
                    low = mid + 1;
                } else {
                    if mid == 0 {
                        break;
                    }
                    high = mid - 1;
                }
            }
            None
        }

        /// Expensive allocation loop.
        pub fn heavy_alloc(env: Env, count: u32) -> u32 {
            let mut v = vec![&env];
            for i in 0..count {
                v.push_back(i);
            }
            v.len()
        }

        /// Lightweight allocation loop.
        pub fn light_alloc(_env: Env, count: u32) -> u32 {
            let mut sum = 0;
            for i in 0..count {
                sum += i;
            }
            sum
        }
    }

    #[test]
    fn test_format_commas_and_signed() {
        assert_eq!(format_commas(0), "0");
        assert_eq!(format_commas(999), "999");
        assert_eq!(format_commas(1000), "1,000");
        assert_eq!(format_commas(1234567), "1,234,567");

        assert_eq!(format_signed_commas(0), "0");
        assert_eq!(format_signed_commas(500), "+500");
        assert_eq!(format_signed_commas(-12500), "-12,500");
    }

    #[test]
    fn test_calculate_savings_percent() {
        // 20% savings
        let p = calculate_savings_percent(100, 80);
        assert!((p - 20.0).abs() < 1e-4);

        // 50% regression
        let r = calculate_savings_percent(100, 150);
        assert!((r - -50.0).abs() < 1e-4);

        // Identical
        assert_eq!(calculate_savings_percent(100, 100), 0.0);

        // Zero baseline
        assert_eq!(calculate_savings_percent(0, 0), 0.0);
        assert_eq!(calculate_savings_percent(0, 100), -100.0);
    }

    #[test]
    fn test_linear_vs_binary_search_gas_savings_passes() {
        let env = Env::default();
        let contract_id = env.register(SearchContract, ());
        let client = SearchContractClient::new(&env, &contract_id);

        let mut items = vec![&env];
        for i in 0..60u64 {
            items.push_back(i * 2);
        }
        let target = 110u64; // Near the end, forcing linear scan to do many iterations

        let (base_res, opt_res) = assert_budget_diff_within!(
            &env,
            baseline: {
                client.linear_search(&items, &target)
            },
            optimized: {
                client.binary_search(&items, &target)
            },
            min_cpu_savings_percent: 20.0,
        );

        assert_eq!(base_res, Some(55));
        assert_eq!(opt_res, Some(55));
    }

    #[test]
    fn test_synthetic_passing_savings() {
        let matcher = BudgetDiffMatcher::new()
            .min_cpu_savings_percent(25.0)
            .min_mem_savings_percent(10.0);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 4_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 70_000, // 30% savings >= 25%
            mem_bytes: 3_200,  // 20% savings >= 10%
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(report.is_satisfied());
        assert!(report.cpu_passed);
        assert!(report.mem_passed);
        assert_eq!(report.cpu_savings, 30_000);
        assert_eq!(report.mem_savings, 800);
    }

    #[test]
    #[should_panic(expected = "Budget Differential Assertion Failed!")]
    fn test_failing_gas_regression_panics() {
        let matcher = BudgetDiffMatcher::new();

        let baseline = BudgetSnapshot {
            cpu_insns: 50_000,
            mem_bytes: 2_000,
        };
        // Optimized path actually regresses and costs MORE!
        let optimized = BudgetSnapshot {
            cpu_insns: 80_000,
            mem_bytes: 3_000,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(!report.is_satisfied());
        report.assert_satisfied();
    }

    #[test]
    #[should_panic(expected = "Failed to achieve target CPU savings")]
    fn test_failing_target_savings_unmet_panics() {
        let matcher = BudgetDiffMatcher::new().min_cpu_savings_percent(50.0);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 2_000,
        };
        // 10% savings achieved, but 50% was required!
        let optimized = BudgetSnapshot {
            cpu_insns: 90_000,
            mem_bytes: 2_000,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(!report.is_satisfied());
        report.assert_satisfied();
    }

    #[test]
    #[should_panic(expected = "Optimized path failed to achieve any gas savings")]
    fn test_failing_zero_savings_panics() {
        let matcher = BudgetDiffMatcher::new();

        let baseline = BudgetSnapshot {
            cpu_insns: 50_000,
            mem_bytes: 2_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 50_000,
            mem_bytes: 2_000,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(!report.is_satisfied());
        report.assert_satisfied();
    }

    #[test]
    fn test_allowed_memory_regression_with_major_cpu_reduction() {
        // Space-time tradeoff: memory increases by 10%, but CPU decreases by 60%
        let matcher = BudgetDiffMatcher::new()
            .min_cpu_savings_percent(50.0)
            .allow_mem_regression(true);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 1_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 40_000,
            mem_bytes: 1_100,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(report.is_satisfied());
        assert!(report.cpu_passed);
        assert!(report.mem_passed);
    }

    #[test]
    fn test_table_formatting_contains_side_by_side_details() {
        let matcher = BudgetDiffMatcher::new().min_cpu_savings_percent(40.0);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 2_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 85_000,
            mem_bytes: 2_500,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        let table = report.format_table();

        assert!(table.contains("Budget Differential Assertion Failed!"));
        assert!(table.contains("CPU Instructions"));
        assert!(table.contains("Memory (Bytes)"));
        assert!(table.contains("100,000"));
        assert!(table.contains("85,000"));
        assert!(table.contains("-15,000"));
        assert!(table.contains("+15.00% savings"));
        assert!(table.contains("+500"));
        assert!(table.contains("FAIL"));
        assert!(table.contains("Failure Summary:"));
    }

    #[test]
    fn test_dual_env_macro() {
        let env1 = Env::default();
        let env2 = Env::default();

        let id1 = env1.register(SearchContract, ());
        let id2 = env2.register(SearchContract, ());

        let client1 = SearchContractClient::new(&env1, &id1);
        let client2 = SearchContractClient::new(&env2, &id2);

        let (res1, res2) = assert_budget_diff_within!(
            baseline: (&env1, { client1.heavy_alloc(&30) }),
            optimized: (&env2, { client2.light_alloc(&30) }),
            min_cpu_savings_percent: 20.0,
        );

        assert_eq!(res1, 30);
        assert_eq!(res2, 435);
    }

    #[test]
    fn test_closure_syntax_macro() {
        let env = Env::default();
        let id = env.register(SearchContract, ());
        let client = SearchContractClient::new(&env, &id);

        let mut items = vec![&env];
        for i in 0..50u64 {
            items.push_back(i * 3);
        }
        let target = 120u64;

        let (b, o) = assert_budget_diff_within!(
            env: &env,
            baseline: || client.linear_search(&items, &target),
            optimized: || client.binary_search(&items, &target),
            min_cpu_savings_percent: 15.0,
        );

        assert_eq!(b, Some(40));
        assert_eq!(o, Some(40));
    }

    #[test]
    #[should_panic(expected = "Budget Differential Assertion Failed!")]
    fn test_macro_panics_on_gas_regression() {
        let env = Env::default();
        let id = env.register(SearchContract, ());
        let client = SearchContractClient::new(&env, &id);

        let mut items = vec![&env];
        for i in 0..50u64 {
            items.push_back(i * 3);
        }
        let target = 120u64;

        // Invert baseline and optimized: optimized is the slow linear search!
        assert_budget_diff_within!(
            &env,
            baseline: {
                client.binary_search(&items, &target)
            },
            optimized: {
                client.linear_search(&items, &target)
            },
        );
    }

    #[test]
    #[should_panic(expected = "Optimized memory exceeded baseline")]
    fn test_failing_memory_regression_panics() {
        let matcher = BudgetDiffMatcher::new();

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 2_000,
        };
        // CPU improves by 50%, but memory regresses by +25%
        let optimized = BudgetSnapshot {
            cpu_insns: 50_000,
            mem_bytes: 2_500,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(!report.is_satisfied());
        report.assert_satisfied();
    }

    #[test]
    fn test_absolute_savings_enforcement() {
        let matcher = BudgetDiffMatcher::new()
            .min_cpu_savings(15_000)
            .min_mem_savings(500);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 3_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 80_000, // 20,000 saved >= 15,000
            mem_bytes: 2_400,  // 600 saved >= 500
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(report.is_satisfied());
        assert_eq!(report.cpu_savings, 20_000);
        assert_eq!(report.mem_savings, 600);
    }

    #[test]
    #[should_panic(expected = "Failed to achieve minimum CPU savings")]
    fn test_absolute_cpu_savings_unmet_panics() {
        let matcher = BudgetDiffMatcher::new().min_cpu_savings(50_000);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 2_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 80_000, // Only 20,000 saved, need 50,000
            mem_bytes: 2_000,
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(!report.is_satisfied());
        report.assert_satisfied();
    }

    #[test]
    #[should_panic(expected = "Failed to achieve minimum memory savings")]
    fn test_absolute_mem_savings_unmet_panics() {
        let matcher = BudgetDiffMatcher::new().min_mem_savings(1_000);

        let baseline = BudgetSnapshot {
            cpu_insns: 100_000,
            mem_bytes: 2_000,
        };
        let optimized = BudgetSnapshot {
            cpu_insns: 60_000,
            mem_bytes: 1_800, // Only 200 saved, need 1,000
        };

        let report = matcher.compare_snapshots(baseline, optimized);
        assert!(!report.is_satisfied());
        report.assert_satisfied();
    }
}
