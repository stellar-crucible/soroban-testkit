//! Comparative budget benchmarking and diff reporting tool.
//!
//! While [`crate::budget::BudgetBaseline`] records CPU and memory costs against fixed baselines,
//! protocol upgrades require comparative diff reports across git revisions or baseline files to detect
//! subtle gas regressions across multiple functions before merging.
//!
//! [`BudgetComparison`] diffs two `budget.json` files, calculates delta CPU instructions,
//! memory bytes, and percentage shifts per function, flags regressions exceeding configurable thresholds
//! (e.g. > 5%), and generates Markdown summary tables and standalone responsive HTML reports.

use crate::budget::{BaselineError, BudgetBaseline, BudgetSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// Status of a function's cost change between baseline and head.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffStatus {
    /// Cost is unchanged across CPU and memory.
    Identical,
    /// CPU or memory consumption decreased without any increase.
    Improved,
    /// CPU or memory consumption increased.
    Regressed,
    /// Function exists in head but not in baseline.
    Added,
    /// Function exists in baseline but not in head.
    Removed,
}

impl DiffStatus {
    /// Short text label with icon.
    pub fn badge(&self) -> &'static str {
        match self {
            DiffStatus::Identical => "⚪ Unchanged",
            DiffStatus::Improved => "🟢 Improved",
            DiffStatus::Regressed => "🔴 Regressed",
            DiffStatus::Added => "🆕 Added",
            DiffStatus::Removed => "🗑️ Removed",
        }
    }
}

/// Budget diff metrics for a single contract function.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionBudgetDiff {
    /// Function or test case name.
    pub function: String,
    /// Baseline cost snapshot, if present.
    pub base: Option<BudgetSnapshot>,
    /// Head / current cost snapshot, if present.
    pub head: Option<BudgetSnapshot>,
    /// CPU instructions difference (`head - base`). Positive means regression.
    pub delta_cpu: i64,
    /// Percentage change in CPU instructions (e.g. `+12.5%` or `-5.0%`).
    pub delta_cpu_percent: f64,
    /// Memory bytes difference (`head - base`). Positive means regression.
    pub delta_mem: i64,
    /// Percentage change in memory bytes.
    pub delta_mem_percent: f64,
    /// Classification of this change.
    pub status: DiffStatus,
}

impl FunctionBudgetDiff {
    /// Returns true if this existing function regressed and either CPU or memory increased by more than `threshold_ratio`
    /// (e.g. `0.05` for 5%).
    pub fn is_regression(&self, threshold_ratio: f64) -> bool {
        if self.status != DiffStatus::Regressed {
            return false;
        }
        let threshold_pct = threshold_ratio * 100.0;
        self.delta_cpu_percent > threshold_pct || self.delta_mem_percent > threshold_pct
    }

    /// Returns true if either CPU or memory decreased without an increase in the other.
    pub fn is_improvement(&self) -> bool {
        (self.delta_cpu < 0 || self.delta_mem < 0) && self.delta_cpu <= 0 && self.delta_mem <= 0
    }
}

/// Comparative budget report diffing two baseline sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BudgetComparison {
    /// Function-level diffs in alphabetical order.
    pub diffs: Vec<FunctionBudgetDiff>,
    /// Total baseline CPU instructions.
    pub total_base_cpu: u64,
    /// Total head CPU instructions.
    pub total_head_cpu: u64,
    /// Total baseline memory bytes.
    pub total_base_mem: u64,
    /// Total head memory bytes.
    pub total_head_mem: u64,
}

impl BudgetComparison {
    /// Compares two [`BudgetBaseline`] instances.
    pub fn diff(base: &BudgetBaseline, head: &BudgetBaseline) -> Self {
        let mut function_names = BTreeSet::new();
        for (name, _) in base.cases() {
            function_names.insert(name.to_string());
        }
        for (name, _) in head.cases() {
            function_names.insert(name.to_string());
        }

        let mut diffs = Vec::new();
        let mut total_base_cpu = 0u64;
        let mut total_head_cpu = 0u64;
        let mut total_base_mem = 0u64;
        let mut total_head_mem = 0u64;

        for name in function_names {
            let base_cost = base.get(&name);
            let head_cost = head.get(&name);

            if let Some(b) = base_cost {
                total_base_cpu = total_base_cpu.saturating_add(b.cpu_insns);
                total_base_mem = total_base_mem.saturating_add(b.mem_bytes);
            }
            if let Some(h) = head_cost {
                total_head_cpu = total_head_cpu.saturating_add(h.cpu_insns);
                total_head_mem = total_head_mem.saturating_add(h.mem_bytes);
            }

            let (delta_cpu, delta_cpu_percent, delta_mem, delta_mem_percent, status) =
                match (base_cost, head_cost) {
                    (Some(b), Some(h)) => {
                        let d_cpu = (h.cpu_insns as i64) - (b.cpu_insns as i64);
                        let pct_cpu = if b.cpu_insns == 0 {
                            if d_cpu > 0 {
                                100.0
                            } else {
                                0.0
                            }
                        } else {
                            (d_cpu as f64 / b.cpu_insns as f64) * 100.0
                        };

                        let d_mem = (h.mem_bytes as i64) - (b.mem_bytes as i64);
                        let pct_mem = if b.mem_bytes == 0 {
                            if d_mem > 0 {
                                100.0
                            } else {
                                0.0
                            }
                        } else {
                            (d_mem as f64 / b.mem_bytes as f64) * 100.0
                        };

                        let st = if d_cpu == 0 && d_mem == 0 {
                            DiffStatus::Identical
                        } else if d_cpu > 0 || d_mem > 0 {
                            DiffStatus::Regressed
                        } else {
                            DiffStatus::Improved
                        };

                        (d_cpu, pct_cpu, d_mem, pct_mem, st)
                    }
                    (None, Some(h)) => (
                        h.cpu_insns as i64,
                        100.0,
                        h.mem_bytes as i64,
                        100.0,
                        DiffStatus::Added,
                    ),
                    (Some(b), None) => (
                        -(b.cpu_insns as i64),
                        -100.0,
                        -(b.mem_bytes as i64),
                        -100.0,
                        DiffStatus::Removed,
                    ),
                    (None, None) => (0, 0.0, 0, 0.0, DiffStatus::Identical),
                };

            diffs.push(FunctionBudgetDiff {
                function: name,
                base: base_cost,
                head: head_cost,
                delta_cpu,
                delta_cpu_percent,
                delta_mem,
                delta_mem_percent,
                status,
            });
        }

        Self {
            diffs,
            total_base_cpu,
            total_head_cpu,
            total_base_mem,
            total_head_mem,
        }
    }

    /// Loads two JSON baseline files from disk and produces a comparison diff.
    pub fn diff_files(
        base_path: impl AsRef<Path>,
        head_path: impl AsRef<Path>,
    ) -> Result<Self, BaselineError> {
        let base = BudgetBaseline::load(base_path.as_ref())?;
        let head = BudgetBaseline::load(head_path.as_ref())?;
        Ok(Self::diff(&base, &head))
    }

    /// Returns all function diffs.
    pub fn diffs(&self) -> &[FunctionBudgetDiff] {
        &self.diffs
    }

    /// Checks if any function cost regressed by more than `threshold` ratio (e.g. `0.05` for 5%).
    pub fn has_regressions_above(&self, threshold: f64) -> bool {
        self.diffs.iter().any(|d| d.is_regression(threshold))
    }

    /// Returns all functions that regressed by more than `threshold` ratio.
    pub fn regressions(&self, threshold: f64) -> Vec<&FunctionBudgetDiff> {
        self.diffs
            .iter()
            .filter(|d| d.is_regression(threshold))
            .collect()
    }

    /// Formats a concise summary of regression findings.
    pub fn summary(&self) -> String {
        let regressed: Vec<_> = self
            .diffs
            .iter()
            .filter(|d| d.status == DiffStatus::Regressed)
            .collect();
        let improved: Vec<_> = self
            .diffs
            .iter()
            .filter(|d| d.status == DiffStatus::Improved)
            .collect();

        if regressed.is_empty() {
            format!(
                "No gas regressions detected across {} functions ({} improved, {} unchanged)",
                self.diffs.len(),
                improved.len(),
                self.diffs.len().saturating_sub(improved.len())
            )
        } else {
            let details = regressed
                .iter()
                .map(|d| {
                    format!(
                        "{}: {:+0.1}% CPU, {:+0.1}% Mem",
                        d.function, d.delta_cpu_percent, d.delta_mem_percent
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            format!("{} regression(s) detected: [{}]", regressed.len(), details)
        }
    }

    /// Generates a clean Markdown table formatted for automated GitHub PR comment injection.
    pub fn to_markdown_table(&self) -> String {
        let mut out = String::new();
        out.push_str("### 📊 Soroban Budget Comparison Report\n\n");
        out.push_str("| Function | Status | CPU (Base → Head) | CPU Shift | Mem (Base → Head) | Mem Shift |\n");
        out.push_str("|:---|:---:|:---:|:---:|:---:|:---:|\n");

        for d in &self.diffs {
            let base_cpu = d
                .base
                .map(|b| format_number(b.cpu_insns))
                .unwrap_or_else(|| "-".into());
            let head_cpu = d
                .head
                .map(|h| format_number(h.cpu_insns))
                .unwrap_or_else(|| "-".into());
            let base_mem = d
                .base
                .map(|b| format_number(b.mem_bytes))
                .unwrap_or_else(|| "-".into());
            let head_mem = d
                .head
                .map(|h| format_number(h.mem_bytes))
                .unwrap_or_else(|| "-".into());

            let cpu_pct = match d.status {
                DiffStatus::Added | DiffStatus::Removed => "-".to_string(),
                _ => format!("{:+0.1}%", d.delta_cpu_percent),
            };
            let mem_pct = match d.status {
                DiffStatus::Added | DiffStatus::Removed => "-".to_string(),
                _ => format!("{:+0.1}%", d.delta_mem_percent),
            };

            out.push_str(&format!(
                "| `{}` | {} | {} → {} | {} | {} → {} | {} |\n",
                d.function,
                d.status.badge(),
                base_cpu,
                head_cpu,
                cpu_pct,
                base_mem,
                head_mem,
                mem_pct
            ));
        }

        out.push('\n');
        let cpu_total_pct = if self.total_base_cpu == 0 {
            0.0
        } else {
            ((self.total_head_cpu as f64 - self.total_base_cpu as f64) / self.total_base_cpu as f64)
                * 100.0
        };
        let mem_total_pct = if self.total_base_mem == 0 {
            0.0
        } else {
            ((self.total_head_mem as f64 - self.total_base_mem as f64) / self.total_base_mem as f64)
                * 100.0
        };

        out.push_str(&format!(
            "**Total Budget Shift:** CPU: {} → {} ({:+0.1}%), Memory: {} → {} ({:+0.1}%)\n",
            format_number(self.total_base_cpu),
            format_number(self.total_head_cpu),
            cpu_total_pct,
            format_number(self.total_base_mem),
            format_number(self.total_head_mem),
            mem_total_pct
        ));

        out
    }

    /// Generates a standalone, responsive HTML report with visual diff bars and styling.
    pub fn to_html_report(&self) -> String {
        let mut rows = String::new();
        for d in &self.diffs {
            let (status_class, status_text) = match d.status {
                DiffStatus::Identical => ("identical", "Unchanged"),
                DiffStatus::Improved => ("improved", "Improved"),
                DiffStatus::Regressed => ("regressed", "Regressed"),
                DiffStatus::Added => ("added", "Added"),
                DiffStatus::Removed => ("removed", "Removed"),
            };

            let base_cpu = d
                .base
                .map(|b| format_number(b.cpu_insns))
                .unwrap_or_else(|| "-".into());
            let head_cpu = d
                .head
                .map(|h| format_number(h.cpu_insns))
                .unwrap_or_else(|| "-".into());
            let base_mem = d
                .base
                .map(|b| format_number(b.mem_bytes))
                .unwrap_or_else(|| "-".into());
            let head_mem = d
                .head
                .map(|h| format_number(h.mem_bytes))
                .unwrap_or_else(|| "-".into());

            let cpu_bar_width = d.delta_cpu_percent.abs().min(100.0);
            let cpu_bar_color = if d.delta_cpu > 0 {
                "#ef4444"
            } else {
                "#22c55e"
            };

            rows.push_str(&format!(
                r#"<tr>
  <td class="fn-name"><code>{}</code></td>
  <td><span class="badge {}">{}</span></td>
  <td class="num">{} → {}</td>
  <td class="num">
    <div class="bar-container">
      <div class="diff-bar" style="width: {}%; background: {};"></div>
      <span>{:+0.1}%</span>
    </div>
  </td>
  <td class="num">{} → {}</td>
  <td class="num">{:+0.1}%</td>
</tr>
"#,
                d.function,
                status_class,
                status_text,
                base_cpu,
                head_cpu,
                cpu_bar_width,
                cpu_bar_color,
                d.delta_cpu_percent,
                base_mem,
                head_mem,
                d.delta_mem_percent
            ));
        }

        format!(
            r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Soroban Budget Comparison Report</title>
  <style>
    body {{
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      margin: 0; padding: 2rem; background: #f8fafc; color: #1e293b;
    }}
    .container {{ max-width: 1000px; margin: 0 auto; background: #fff; border-radius: 8px; box-shadow: 0 1px 3px rgba(0,0,0,0.1); padding: 2rem; }}
    h1 {{ margin-top: 0; font-size: 1.5rem; border-bottom: 2px solid #e2e8f0; padding-bottom: 0.75rem; }}
    .summary-cards {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(200px, 1fr)); gap: 1rem; margin-bottom: 2rem; }}
    .card {{ background: #f1f5f9; padding: 1rem; border-radius: 6px; }}
    .card-title {{ font-size: 0.875rem; color: #64748b; }}
    .card-val {{ font-size: 1.25rem; font-weight: bold; margin-top: 0.25rem; }}
    table {{ width: 100%; border-collapse: collapse; margin-top: 1rem; }}
    th, td {{ padding: 0.75rem 1rem; text-align: left; border-bottom: 1px solid #e2e8f0; }}
    th {{ background: #f8fafc; font-weight: 600; font-size: 0.875rem; color: #475569; }}
    .num {{ text-align: right; }}
    .fn-name {{ font-weight: 600; }}
    .badge {{ display: inline-block; padding: 0.2rem 0.5rem; border-radius: 9999px; font-size: 0.75rem; font-weight: 600; }}
    .badge.improved {{ background: #dcfce7; color: #15803d; }}
    .badge.regressed {{ background: #fee2e2; color: #b91c1c; }}
    .badge.identical {{ background: #f1f5f9; color: #475569; }}
    .badge.added {{ background: #e0f2fe; color: #0369a1; }}
    .badge.removed {{ background: #fef3c7; color: #b45309; }}
    .bar-container {{ display: flex; align-items: center; justify-content: flex-end; gap: 0.5rem; }}
    .diff-bar {{ height: 6px; border-radius: 3px; }}
  </style>
</head>
<body>
  <div class="container">
    <h1>Soroban Budget Comparison Report</h1>
    <div class="summary-cards">
      <div class="card">
        <div class="card-title">Functions Diffed</div>
        <div class="card-val">{}</div>
      </div>
      <div class="card">
        <div class="card-title">Total Base CPU</div>
        <div class="card-val">{}</div>
      </div>
      <div class="card">
        <div class="card-title">Total Head CPU</div>
        <div class="card-val">{}</div>
      </div>
    </div>
    <table>
      <thead>
        <tr>
          <th>Function</th>
          <th>Status</th>
          <th class="num">CPU (Base → Head)</th>
          <th class="num">CPU Shift</th>
          <th class="num">Memory (Base → Head)</th>
          <th class="num">Mem Shift</th>
        </tr>
      </thead>
      <tbody>
        {}
      </tbody>
    </table>
  </div>
</body>
</html>
"#,
            self.diffs.len(),
            format_number(self.total_base_cpu),
            format_number(self.total_head_cpu),
            rows
        )
    }
}

fn format_number(val: u64) -> String {
    let s = val.to_string();
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    for (i, c) in chars.into_iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::BudgetSnapshot;

    #[test]
    fn test_budget_comparison_diff_scenarios() {
        let mut base = BudgetBaseline::new();
        let mut head = BudgetBaseline::new();

        // 1. Identical
        base.record(
            "identical_fn",
            BudgetSnapshot {
                cpu_insns: 100_000,
                mem_bytes: 5_000,
            },
        );
        head.record(
            "identical_fn",
            BudgetSnapshot {
                cpu_insns: 100_000,
                mem_bytes: 5_000,
            },
        );

        // 2. Improved
        base.record(
            "improved_fn",
            BudgetSnapshot {
                cpu_insns: 200_000,
                mem_bytes: 10_000,
            },
        );
        head.record(
            "improved_fn",
            BudgetSnapshot {
                cpu_insns: 150_000,
                mem_bytes: 8_000,
            },
        );

        // 3. Regressed
        base.record(
            "regressed_fn",
            BudgetSnapshot {
                cpu_insns: 100_000,
                mem_bytes: 5_000,
            },
        );
        head.record(
            "regressed_fn",
            BudgetSnapshot {
                cpu_insns: 120_000,
                mem_bytes: 6_000,
            },
        );

        // 4. Added
        head.record(
            "added_fn",
            BudgetSnapshot {
                cpu_insns: 50_000,
                mem_bytes: 2_000,
            },
        );

        // 5. Removed
        base.record(
            "removed_fn",
            BudgetSnapshot {
                cpu_insns: 30_000,
                mem_bytes: 1_000,
            },
        );

        let comparison = BudgetComparison::diff(&base, &head);
        assert_eq!(comparison.diffs().len(), 5);

        let identical = comparison
            .diffs()
            .iter()
            .find(|d| d.function == "identical_fn")
            .unwrap();
        assert_eq!(identical.status, DiffStatus::Identical);
        assert_eq!(identical.delta_cpu, 0);

        let improved = comparison
            .diffs()
            .iter()
            .find(|d| d.function == "improved_fn")
            .unwrap();
        assert_eq!(improved.status, DiffStatus::Improved);
        assert!(improved.delta_cpu < 0);
        assert!(improved.is_improvement());

        let regressed = comparison
            .diffs()
            .iter()
            .find(|d| d.function == "regressed_fn")
            .unwrap();
        assert_eq!(regressed.status, DiffStatus::Regressed);
        assert!(regressed.delta_cpu > 0);
        assert!(regressed.is_regression(0.05)); // > 5% regression

        let added = comparison
            .diffs()
            .iter()
            .find(|d| d.function == "added_fn")
            .unwrap();
        assert_eq!(added.status, DiffStatus::Added);

        let removed = comparison
            .diffs()
            .iter()
            .find(|d| d.function == "removed_fn")
            .unwrap();
        assert_eq!(removed.status, DiffStatus::Removed);

        assert!(comparison.has_regressions_above(0.05));
        assert_eq!(comparison.regressions(0.05).len(), 1);

        // Markdown table check
        let md = comparison.to_markdown_table();
        assert!(md.contains("identical_fn"));
        assert!(md.contains("improved_fn"));
        assert!(md.contains("regressed_fn"));
        assert!(md.contains("added_fn"));
        assert!(md.contains("removed_fn"));

        // HTML report check
        let html = comparison.to_html_report();
        assert!(html.contains("Soroban Budget Comparison Report"));
        assert!(html.contains("regressed_fn"));
    }
}
