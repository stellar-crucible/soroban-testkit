//! `assert_budget_consumed!`: bound what one invocation cost, in CPU
//! instructions and memory bytes, with a range or a ceiling (issue #107).
//!
//! The measurement is [`BudgetSnapshot::last_invocation`] from
//! `soroban-testkit-core`, so the macro judges the most recent top-level call,
//! not a running total. Each metric takes either a range (`cpu: 10_000..=15_000`,
//! any standard range form) or a ceiling (`max_cpu: 20_000`, inclusive). A
//! breach reports the observed value, the expected bound, the amount it missed
//! by, and that amount as a percentage of the bound it crossed.

use soroban_testkit_core::budget::BudgetSnapshot;
use std::fmt;
use std::ops::{Bound, RangeBounds};

/// Inclusive lower and upper limits for one metric. `None` means unbounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetricBound {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

impl MetricBound {
    /// Normalise any standard range over `u64` to inclusive limits.
    pub fn from_range(range: impl RangeBounds<u64>) -> Self {
        let min = match range.start_bound() {
            Bound::Included(&n) => Some(n),
            Bound::Excluded(&n) => Some(n.saturating_add(1)),
            Bound::Unbounded => None,
        };
        let max = match range.end_bound() {
            Bound::Included(&n) => Some(n),
            Bound::Excluded(&n) => Some(n.saturating_sub(1)),
            Bound::Unbounded => None,
        };
        Self { min, max }
    }

    /// An inclusive ceiling with no floor.
    pub fn at_most(limit: u64) -> Self {
        Self {
            min: None,
            max: Some(limit),
        }
    }

    fn render(&self) -> String {
        match (self.min, self.max) {
            (Some(min), Some(max)) => format!("{}..={}", group(min), group(max)),
            (Some(min), None) => format!("{}..", group(min)),
            (None, Some(max)) => format!("..={}", group(max)),
            (None, None) => "..".to_string(),
        }
    }
}

/// The metric a breach was measured on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsumedMetric {
    Cpu,
    Memory,
}

impl ConsumedMetric {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConsumedMetric::Cpu => "cpu_insns",
            ConsumedMetric::Memory => "mem_bytes",
        }
    }
}

/// Which side of the bound was crossed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreachSide {
    /// Consumed less than the range's lower bound.
    Below,
    /// Consumed more than the range's upper bound or the ceiling.
    Above,
}

/// One metric outside its bound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetBreach {
    pub metric: ConsumedMetric,
    pub observed: u64,
    pub expected: MetricBound,
    pub side: BreachSide,
    /// How far past the crossed bound the observation is.
    pub amount: u64,
    /// The crossed bound itself, for the percentage deviation.
    pub bound: u64,
}

impl BudgetBreach {
    /// `amount` as a percentage of the crossed bound (100% when the bound is 0).
    pub fn deviation_percent(&self) -> f64 {
        if self.bound == 0 {
            100.0
        } else {
            self.amount as f64 * 100.0 / self.bound as f64
        }
    }
}

impl fmt::Display for BudgetBreach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (side, sign) = match self.side {
            BreachSide::Above => ("over the upper bound", '+'),
            BreachSide::Below => ("under the lower bound", '-'),
        };
        write!(
            f,
            "{}: observed {}, expected {} — {} by {} ({}{:.2}%)",
            self.metric.as_str(),
            group(self.observed),
            self.expected.render(),
            side,
            group(self.amount),
            sign,
            self.deviation_percent()
        )
    }
}

/// Expected CPU and memory consumption for one invocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BudgetBounds {
    pub cpu: MetricBound,
    pub mem: MetricBound,
}

impl BudgetBounds {
    pub fn new() -> Self {
        Self::default()
    }

    /// CPU instructions must fall inside `range`.
    pub fn cpu(mut self, range: impl RangeBounds<u64>) -> Self {
        self.cpu = MetricBound::from_range(range);
        self
    }

    /// CPU instructions must not exceed `limit` (inclusive).
    pub fn max_cpu(mut self, limit: u64) -> Self {
        self.cpu = MetricBound::at_most(limit);
        self
    }

    /// Memory bytes must fall inside `range`.
    pub fn mem(mut self, range: impl RangeBounds<u64>) -> Self {
        self.mem = MetricBound::from_range(range);
        self
    }

    /// Memory bytes must not exceed `limit` (inclusive).
    pub fn max_mem(mut self, limit: u64) -> Self {
        self.mem = MetricBound::at_most(limit);
        self
    }

    /// Every bound the snapshot breaks, CPU first.
    pub fn breaches(&self, consumed: &BudgetSnapshot) -> Vec<BudgetBreach> {
        [
            (ConsumedMetric::Cpu, consumed.cpu_insns, self.cpu),
            (ConsumedMetric::Memory, consumed.mem_bytes, self.mem),
        ]
        .into_iter()
        .filter_map(|(metric, observed, expected)| breach(metric, observed, expected))
        .collect()
    }

    /// `Ok` when the snapshot is within every bound, otherwise the full report.
    pub fn check(&self, consumed: &BudgetSnapshot) -> Result<(), String> {
        let breaches = self.breaches(consumed);
        if breaches.is_empty() {
            return Ok(());
        }
        let mut report = String::from("assert_budget_consumed! failed:\n");
        for b in &breaches {
            report.push_str("  ");
            report.push_str(&b.to_string());
            report.push('\n');
        }
        report.push_str(&format!(
            "  observed totals: cpu_insns={} mem_bytes={}",
            group(consumed.cpu_insns),
            group(consumed.mem_bytes)
        ));
        Err(report)
    }

    /// Panic with every breach at once.
    #[track_caller]
    pub fn assert_within(&self, consumed: &BudgetSnapshot) {
        if let Err(report) = self.check(consumed) {
            panic!("{report}");
        }
    }
}

fn breach(metric: ConsumedMetric, observed: u64, expected: MetricBound) -> Option<BudgetBreach> {
    if let Some(max) = expected.max {
        if observed > max {
            return Some(BudgetBreach {
                metric,
                observed,
                expected,
                side: BreachSide::Above,
                amount: observed - max,
                bound: max,
            });
        }
    }
    if let Some(min) = expected.min {
        if observed < min {
            return Some(BudgetBreach {
                metric,
                observed,
                expected,
                side: BreachSide::Below,
                amount: min - observed,
                bound: min,
            });
        }
    }
    None
}

/// `1234567` -> `1_234_567`, matching how limits are written in Rust source.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(ch);
    }
    out
}

/// The cost of the most recent top-level invocation on `env`.
pub fn last_invocation(env: &soroban_sdk::Env) -> BudgetSnapshot {
    BudgetSnapshot::last_invocation(env)
}

/// Assert what the most recent invocation on an `Env` consumed.
///
/// Keys: `cpu` / `mem` take any `u64` range (`a..b`, `a..=b`, `a..`, `..b`,
/// `..=b`); `max_cpu` / `max_mem` take an inclusive ceiling. Unknown keys are a
/// compile error. Call it after the invocation you mean to measure.
///
/// ```ignore
/// client.swap(&user, &100);
/// assert_budget_consumed!(env, {
///     cpu: 10_000..=150_000,
///     max_mem: 50_000,
/// });
/// ```
#[macro_export]
macro_rules! assert_budget_consumed {
    ($env:expr, { $($key:ident : $value:expr),* $(,)? }) => {{
        let bounds = $crate::budget::BudgetBounds::new();
        $( let bounds = $crate::__budget_consumed_key!(bounds, $key, $value); )*
        bounds.assert_within(&$crate::budget::last_invocation(&$env));
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __budget_consumed_key {
    ($bounds:expr, cpu, $value:expr) => {
        $bounds.cpu($value)
    };
    ($bounds:expr, max_cpu, $value:expr) => {
        $bounds.max_cpu($value)
    };
    ($bounds:expr, mem, $value:expr) => {
        $bounds.mem($value)
    };
    ($bounds:expr, max_mem, $value:expr) => {
        $bounds.max_mem($value)
    };
    ($bounds:expr, $other:ident, $value:expr) => {
        compile_error!(concat!(
            "assert_budget_consumed!: unknown key `",
            stringify!($other),
            "`; expected cpu, max_cpu, mem or max_mem"
        ))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env};

    fn snapshot(cpu: u64, mem: u64) -> BudgetSnapshot {
        BudgetSnapshot {
            cpu_insns: cpu,
            mem_bytes: mem,
        }
    }

    #[test]
    fn passes_inside_ranges_and_ceilings() {
        let s = snapshot(12_000, 3_000);
        assert!(BudgetBounds::new().cpu(10_000..=15_000).check(&s).is_ok());
        assert!(BudgetBounds::new().cpu(10_000..15_000).check(&s).is_ok());
        assert!(BudgetBounds::new().max_cpu(12_000).check(&s).is_ok());
        assert!(BudgetBounds::new()
            .mem(3_000..)
            .max_cpu(20_000)
            .check(&s)
            .is_ok());
    }

    #[test]
    fn reports_an_upper_bound_breach_with_percentage() {
        let err = BudgetBounds::new()
            .max_cpu(1_500)
            .check(&snapshot(1_650, 0))
            .unwrap_err();
        assert!(
            err.contains("cpu_insns: observed 1_650, expected ..=1_500"),
            "{err}"
        );
        assert!(
            err.contains("over the upper bound by 150 (+10.00%)"),
            "{err}"
        );
    }

    #[test]
    fn reports_a_lower_bound_breach() {
        let err = BudgetBounds::new()
            .mem(1_000..2_000)
            .check(&snapshot(0, 900))
            .unwrap_err();
        assert!(
            err.contains("mem_bytes: observed 900, expected 1_000..=1_999"),
            "{err}"
        );
        assert!(
            err.contains("under the lower bound by 100 (-10.00%)"),
            "{err}"
        );
    }

    #[test]
    fn exclusive_range_end_is_exclusive() {
        let breaches = BudgetBounds::new()
            .cpu(1_000..2_000)
            .breaches(&snapshot(2_000, 0));
        assert_eq!(breaches.len(), 1);
        assert_eq!(breaches[0].side, BreachSide::Above);
        assert_eq!(breaches[0].amount, 1);
    }

    #[test]
    fn reports_every_breach_at_once() {
        let breaches = BudgetBounds::new()
            .max_cpu(10)
            .max_mem(10)
            .breaches(&snapshot(11, 12));
        assert_eq!(
            breaches.iter().map(|b| b.metric).collect::<Vec<_>>(),
            vec![ConsumedMetric::Cpu, ConsumedMetric::Memory]
        );
    }

    #[contract]
    struct Meter;

    #[contractimpl]
    impl Meter {
        pub fn work(_env: Env, n: u32) -> u64 {
            (0..n as u64).fold(0u64, |acc, x| acc.wrapping_add(x * x))
        }
    }

    #[test]
    fn macro_passes_on_a_generous_ceiling() {
        let env = Env::default();
        let client = MeterClient::new(&env, &env.register(Meter, ()));
        client.work(&10);
        assert_budget_consumed!(env, {
            max_cpu: u64::MAX,
            max_mem: u64::MAX,
        });
        assert_budget_consumed!(env, { cpu: 1.., mem: 1.. });
    }

    #[test]
    #[should_panic(expected = "over the upper bound")]
    fn macro_fails_on_a_ceiling_breach() {
        let env = Env::default();
        let client = MeterClient::new(&env, &env.register(Meter, ()));
        client.work(&10);
        assert_budget_consumed!(env, { max_cpu: 1 });
    }

    #[test]
    #[should_panic(expected = "under the lower bound")]
    fn macro_fails_on_a_lower_bound_breach() {
        let env = Env::default();
        let client = MeterClient::new(&env, &env.register(Meter, ()));
        client.work(&10);
        assert_budget_consumed!(env, { cpu: u64::MAX - 1.. });
    }
}
