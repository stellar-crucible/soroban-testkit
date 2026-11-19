use soroban_sdk::testutils::BudgetSnapshot;

/// Which of the two ledger resources a violation was measured against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BudgetMetric {
    Cpu,
    Memory,
}

impl BudgetMetric {
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetMetric::Cpu => "cpu_insns",
            BudgetMetric::Memory => "mem_bytes",
        }
    }
}

/// Why a cost was rejected: over an absolute ceiling, or grown past a baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViolationKind {
    Ceiling,
    Growth {
        baseline: u64,
        tolerance_percent: u64,
    },
}

/// One rejection, rendered as `key=value` pairs for CI to parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetViolation {
    pub case: String,
    pub metric: BudgetMetric,
    pub actual: u64,
    pub limit: u64,
    pub kind: ViolationKind,
}

impl std::fmt::Display for BudgetViolation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            ViolationKind::Ceiling => write!(
                formatter,
                "BUDGET kind=ceiling case={} metric={} actual={} limit={}",
                self.case, self.metric.as_str(), self.actual, self.limit
            ),
            ViolationKind::Growth {
                baseline,
                tolerance_percent,
            } => write!(
                formatter,
                "BUDGET kind=growth case={} metric={} actual={} limit={} baseline={} tolerance_percent={}",
                self.case,
                self.metric.as_str(),
                self.actual,
                self.limit,
                baseline,
                tolerance_percent
            ),
        }
    }
}

/// A guard for asserting budget consumption with ceiling and tolerance.
#[derive(Clone, Debug)]
pub struct BudgetGuard {
    case: String,
    cpu_ceiling: Option<u64>,
    mem_ceiling: Option<u64>,
    baseline: Option<BudgetSnapshot>,
    tolerance_percent: u64,
}

impl BudgetGuard {
    pub fn new(case: impl Into<String>) -> Self {
        Self {
            case: case.into(),
            cpu_ceiling: None,
            mem_ceiling: None,
            baseline: None,
            tolerance_percent: 0,
        }
    }

    pub fn cpu_ceiling(mut self, instructions: u64) -> Self {
        self.cpu_ceiling = Some(instructions);
        self
    }

    pub fn mem_ceiling(mut self, bytes: u64) -> Self {
        self.mem_ceiling = Some(bytes);
        self
    }

    /// Compare against a recorded cost, rejecting anything that grew past
    /// `tolerance_percent` of it. A baseline of `None` — the case is not in the
    /// file yet — leaves the ceilings as the only constraint.
    pub fn baseline(mut self, baseline: Option<BudgetSnapshot>) -> Self {
        self.baseline = baseline;
        self
    }

    /// How much a metric may grow before the guard rejects it. Defaults to 0,
    /// which means "not one instruction more than the baseline".
    pub fn tolerance_percent(mut self, percent: u64) -> Self {
        self.tolerance_percent = percent;
        self
    }

    /// Set the growth tolerance percent directly (alias for `tolerance_percent`).
    pub fn growth_tolerance(mut self, percent: u64) -> Self {
        self.tolerance_percent = percent;
        self
    }

    /// Every limit this cost breaks, ceilings first then growth, CPU first.
    pub fn violations(&self, cost: &BudgetSnapshot) -> Vec<BudgetViolation> {
        let mut found = Vec::new();
        let metrics = [
            (BudgetMetric::Cpu, cost.cpu_insns, self.cpu_ceiling),
            (BudgetMetric::Memory, cost.mem_bytes, self.mem_ceiling),
        ];

        for (metric, actual, ceiling) in metrics {
            if let Some(limit) = ceiling {
                if actual > limit {
                    found.push(BudgetViolation {
                        case: self.case.clone(),
                        metric,
                        actual,
                        limit,
                        kind: ViolationKind::Ceiling,
                    });
                }
            }
        }

        if let Some(baseline) = self.baseline {
            let growth = [
                (BudgetMetric::Cpu, cost.cpu_insns, baseline.cpu_insns),
                (BudgetMetric::Memory, cost.mem_bytes, baseline.mem_bytes),
            ];
            for (metric, actual, start) in growth {
                let limit = allowed(start, self.tolerance_percent);
                if actual > limit {
                    found.push(BudgetViolation {
                        case: self.case.clone(),
                        metric,
                        actual,
                        limit,
                        kind: ViolationKind::Growth {
                            baseline: start,
                            tolerance_percent: self.tolerance_percent,
                        },
                    });
                }
            }
        }

        found
    }

    /// Fail the test with one line per breach, all of them at once.
    ///
    /// Reporting every breach rather than the first is what makes the output
    /// useful in CI: a change that costs both CPU and memory should not have to
    /// be discovered twice.
    pub fn assert_within(&self, cost: &BudgetSnapshot) {
        let violations = self.violations(cost);
        if !violations.is_empty() {
            let report = violations
                .iter()
                .map(|violation| violation.to_string())
                .collect::<Vec<_>>()
                .join("\n");
            panic!("{report}");
        }
    }

    /// Run an invocation, measure what it cost, and assert the measurement.
    ///
    /// The measurement is [`BudgetSnapshot::last_invocation`], so it belongs to
    /// the call the closure made. A closure that makes several calls is judged
    /// on the last one — measure a sequence by wrapping each call in its own
    /// guard.
    pub fn run<T>(&self, env: &soroban_sdk::Env, invocation: impl FnOnce() -> T) -> T {
        let value = invocation();

        self.assert_within(&BudgetSnapshot::last_invocation(env));
        value
    }
}

/// Measure one invocation against a [`BudgetGuard`] without naming the guard.
///
/// `case` labels the violation lines, `config` holds the limits, and
/// `invocation` is a closure run once and measured. The macro takes the same
/// arguments the builder does, and is a shorthand for one call site; a test
/// that loops over cases should build a [`BudgetGuard`] instead.
#[macro_export]
macro_rules! assert_budget_consumed! {
    ($env:expr, { $($config:tt)* }, $invocation:expr $(,)?) => {{
        let guard = $crate::budget_guard!(@limit $crate::budget::BudgetGuard::new(stringify!($case)), { $($config)* });
        guard.run($env, $invocation)
    }};

    (@limit $guard:expr, {}) => { $guard };
    (@limit $guard:expr, { cpu: $range:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.cpu_ceiling($range.start), { $($($rest)*)? })
    };
    (@limit $guard:expr, { mem: $range:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.mem_ceiling($range.start), { $($($rest)*)? })
    };
    (@limit $guard:expr, { max_cpu: $limit:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.cpu_ceiling($limit), { $($($rest)*)? })
    };
    (@limit $guard:expr, { max_mem: $limit:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.mem_ceiling($limit), { $($($rest)*)? })
    };
    (@limit $guard:expr, { baseline: $baseline:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.baseline($baseline), { $($($rest)*)? })
    };
    (@limit $guard:expr, { tolerance: $percent:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.tolerance($percent), { $($($rest)*)? })
    };
    (@limit $guard:expr, { tolerance_percent: $percent:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.tolerance_percent($percent), { $($($rest)*)? })
    };
}

/// The largest cost still accepted for a baseline grown by `percent`.
///
/// The allowance rounds down, so a 10% tolerance on a 15-instruction baseline
/// admits 16, not 17 — a guard should be the stricter of two readings.
fn allowed(baseline: u64, percent: u64) -> u64 {
    let growth = (baseline as u128 * percent as u128) / 100;
    baseline.saturating_add(growth.min(u128::from(u64::MAX)) as u64)
}