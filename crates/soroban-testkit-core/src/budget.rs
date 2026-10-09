use serde::{Deserialize, Serialize};
use soroban_env_host::budget::Budget;
use std::collections::BTreeMap;

/// Anything Testkit can read consumed CPU and memory from.
///
/// Implemented for the host [`Budget`] and for the SDK budget returned by
/// `env.cost_estimate().budget()`, so a snapshot can be captured from either.
pub trait BudgetRead {
    fn cpu_insns(&self) -> u64;
    fn mem_bytes(&self) -> u64;
}

impl BudgetRead for Budget {
    fn cpu_insns(&self) -> u64 {
        self.get_cpu_insns_consumed().unwrap_or(0)
    }

    fn mem_bytes(&self) -> u64 {
        self.get_mem_bytes_consumed().unwrap_or(0)
    }
}

impl BudgetRead for soroban_sdk::testutils::budget::Budget {
    fn cpu_insns(&self) -> u64 {
        self.cpu_instruction_cost()
    }

    fn mem_bytes(&self) -> u64 {
        self.memory_bytes_cost()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetSnapshot {
    pub cpu_insns: u64,
    pub mem_bytes: u64,
}

impl BudgetSnapshot {
    pub fn capture(budget: &impl BudgetRead) -> Self {
        Self {
            cpu_insns: budget.cpu_insns(),
            mem_bytes: budget.mem_bytes(),
        }
    }

    pub fn diff(&self, other: &BudgetSnapshot) -> BudgetSnapshot {
        BudgetSnapshot {
            cpu_insns: other.cpu_insns.saturating_sub(self.cpu_insns),
            mem_bytes: other.mem_bytes.saturating_sub(self.mem_bytes),
        }
    }
}

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

/// A ceiling and an optional baseline for one named operation.
///
/// Cost alone tells you nothing; what a test wants is "this call stayed inside
/// its budget, and it did not grow relative to the last time we measured it".
/// The guard states both, and renders either failure as one parseable line.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Reads the budget through `env.cost_estimate()`, so the measurement
    /// includes the estimator's own work — a small, fixed overhead that a
    /// baseline absorbs because it was measured the same way.
    pub fn run<T>(&self, env: &soroban_sdk::Env, invocation: impl FnOnce() -> T) -> T {
        let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
        let value = invocation();
        let after = BudgetSnapshot::capture(&env.cost_estimate().budget());

        self.assert_within(&before.diff(&after));
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
macro_rules! budget_guard {
    ($env:expr, $case:expr, { $($config:tt)* }, $invocation:expr $(,)?) => {{
        let guard = $crate::budget_guard!(@limit $crate::budget::BudgetGuard::new($case), { $($config)* });
        guard.run($env, $invocation)
    }};

    (@limit $guard:expr, {}) => { $guard };
    (@limit $guard:expr, { cpu_max: $limit:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.cpu_ceiling($limit), { $($($rest)*)? })
    };
    (@limit $guard:expr, { mem_max: $limit:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.mem_ceiling($limit), { $($($rest)*)? })
    };
    (@limit $guard:expr, { baseline: $baseline:expr $(, $($rest:tt)*)? }) => {
        $crate::budget_guard!(@limit $guard.baseline($baseline), { $($($rest)*)? })
    };
    (@limit $guard:expr, { tolerance: $percent:expr $(, $($rest:tt)*)? }) => {
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

/// Costs recorded by an earlier run, kept in a file CI can commit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetBaseline {
    cases: BTreeMap<String, BudgetSnapshot>,
}

/// A baseline file the loader understands.
const BASELINE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct BaselineFile {
    version: u32,
    cases: BTreeMap<String, BudgetSnapshot>,
}

/// Why a baseline file could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaselineError {
    Io(String),
    Parse(String),
    UnsupportedVersion { found: u32 },
}

impl std::fmt::Display for BaselineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BaselineError::Io(reason) => write!(formatter, "cannot read the baseline file: {reason}"),
            BaselineError::Parse(reason) => {
                write!(formatter, "baseline file is not valid: {reason}")
            }
            BaselineError::UnsupportedVersion { found } => write!(
                formatter,
                "baseline file version {found} is newer than this testkit understands ({BASELINE_VERSION})"
            ),
        }
    }
}

impl std::error::Error for BaselineError {}

impl BudgetBaseline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(path: &std::path::Path) -> Result<Self, BaselineError> {
        let text =
            std::fs::read_to_string(path).map_err(|error| BaselineError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, BaselineError> {
        let file: BaselineFile =
            serde_json::from_str(text).map_err(|error| BaselineError::Parse(error.to_string()))?;
        if file.version > BASELINE_VERSION {
            return Err(BaselineError::UnsupportedVersion {
                found: file.version,
            });
        }
        Ok(Self { cases: file.cases })
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), BaselineError> {
        std::fs::write(path, self.to_json_string().as_bytes())
            .map_err(|error| BaselineError::Io(error.to_string()))
    }

    pub fn to_json_string(&self) -> String {
        let file = BaselineFile {
            version: BASELINE_VERSION,
            cases: self.cases.clone(),
        };
        // Serialising a version integer and a map of two u64s cannot fail.
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    pub fn record(&mut self, case: impl Into<String>, cost: BudgetSnapshot) -> &mut Self {
        self.cases.insert(case.into(), cost);
        self
    }

    pub fn get(&self, case: &str) -> Option<BudgetSnapshot> {
        self.cases.get(case).copied()
    }

    pub fn cases(&self) -> impl Iterator<Item = (&str, BudgetSnapshot)> {
        self.cases.iter().map(|(name, cost)| (name.as_str(), *cost))
    }

    pub fn len(&self) -> usize {
        self.cases.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// A guard for `case`, pre-loaded with its recorded cost if the file has one.
    pub fn guard(&self, case: &str) -> BudgetGuard {
        BudgetGuard::new(case).baseline(self.get(case))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        allowed, BaselineError, BudgetBaseline, BudgetGuard, BudgetMetric, BudgetSnapshot,
        ViolationKind, BASELINE_VERSION,
    };
    use soroban_env_host::{budget::Budget, xdr::ContractCostType};
    use soroban_sdk::{contract, contractimpl, Env, Symbol};

    fn charged_budget() -> Budget {
        let budget = Budget::default();
        budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        budget
    }

    #[test]
    fn capture_accepts_the_sdk_budget_of_an_untouched_environment() {
        let env = soroban_sdk::Env::default();
        let snapshot = BudgetSnapshot::capture(&env.cost_estimate().budget());

        // An environment that has not run a contract call estimates nothing;
        // examples/counter covers the case of a call that consumes budget.
        assert_eq!(snapshot.cpu_insns, 0);
        assert_eq!(snapshot.mem_bytes, 0);
    }

    #[test]
    fn capture_on_idle_budget_reports_zero() {
        let snapshot = BudgetSnapshot::capture(&Budget::default());
        assert_eq!(snapshot.cpu_insns, 0);
        assert_eq!(snapshot.mem_bytes, 0);
    }

    #[test]
    fn capture_reflects_consumed_resources() {
        let snapshot = BudgetSnapshot::capture(&charged_budget());
        assert!(
            snapshot.cpu_insns > 0,
            "charging the budget must register CPU consumption"
        );
    }

    #[test]
    fn diff_is_monotonic_between_captures() {
        let budget = Budget::default();
        let before = BudgetSnapshot::capture(&budget);
        for _ in 0..5 {
            budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        }
        let after = BudgetSnapshot::capture(&budget);

        let delta = before.diff(&after);
        assert_eq!(delta.cpu_insns, after.cpu_insns - before.cpu_insns);
        assert!(delta.cpu_insns > 0);
    }

    #[test]
    fn diff_saturates_when_the_later_snapshot_is_smaller() {
        let before = BudgetSnapshot {
            cpu_insns: 900,
            mem_bytes: 400,
        };
        let after = BudgetSnapshot {
            cpu_insns: 100,
            mem_bytes: 50,
        };

        let delta = before.diff(&after);
        assert_eq!(delta.cpu_insns, 0);
        assert_eq!(delta.mem_bytes, 0);
    }

    #[test]
    fn diff_of_identical_snapshots_is_zero() {
        let snapshot = BudgetSnapshot::capture(&charged_budget());
        let delta = snapshot.diff(&snapshot);
        assert_eq!(delta.cpu_insns, 0);
        assert_eq!(delta.mem_bytes, 0);
    }

    fn cost(cpu_insns: u64, mem_bytes: u64) -> BudgetSnapshot {
        BudgetSnapshot {
            cpu_insns,
            mem_bytes,
        }
    }

    #[test]
    fn a_cost_inside_every_limit_violates_nothing() {
        let guard = BudgetGuard::new("transfer")
            .cpu_ceiling(1_000)
            .mem_ceiling(500)
            .baseline(Some(cost(1_000, 500)))
            .tolerance_percent(10);

        assert!(guard.violations(&cost(900, 400)).is_empty());
    }

    #[test]
    fn a_ceiling_violation_names_the_metric_and_the_limit() {
        let violations = BudgetGuard::new("transfer")
            .cpu_ceiling(1_000)
            .violations(&cost(1_500, 0));

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].metric, BudgetMetric::Cpu);
        assert_eq!(violations[0].actual, 1_500);
        assert_eq!(violations[0].limit, 1_000);
        assert_eq!(violations[0].kind, ViolationKind::Ceiling);
    }

    #[test]
    fn every_breach_is_reported_in_one_pass() {
        let violations = BudgetGuard::new("transfer")
            .cpu_ceiling(1_000)
            .mem_ceiling(500)
            .baseline(Some(cost(100, 100)))
            .tolerance_percent(0)
            .violations(&cost(2_000, 600));

        assert_eq!(violations.len(), 4, "two ceilings and two growths");
        assert_eq!(violations[0].metric, BudgetMetric::Cpu);
        assert_eq!(violations[1].metric, BudgetMetric::Memory);
        assert_eq!(violations[2].metric, BudgetMetric::Cpu);
        assert_eq!(violations[3].metric, BudgetMetric::Memory);
    }

    #[test]
    fn growth_is_measured_against_the_tolerance() {
        let guard = || {
            BudgetGuard::new("transfer")
                .baseline(Some(cost(1_000, 1_000)))
                .tolerance_percent(10)
        };

        assert!(guard().violations(&cost(1_100, 1_100)).is_empty());
        let violations = guard().violations(&cost(1_101, 1_000));
        assert_eq!(violations.len(), 1);
        assert_eq!(
            violations[0].kind,
            ViolationKind::Growth {
                baseline: 1_000,
                tolerance_percent: 10
            }
        );
    }

    #[test]
    fn a_missing_baseline_leaves_ceilings_in_force() {
        let guard = BudgetGuard::new("transfer")
            .cpu_ceiling(1_000)
            .baseline(None)
            .tolerance_percent(10);

        assert!(guard.violations(&cost(1_000_000, 0)).len() == 1);
    }

    #[test]
    fn tolerance_of_zero_rejects_any_growth_at_all() {
        let violations = BudgetGuard::new("transfer")
            .baseline(Some(cost(1_000, 1_000)))
            .violations(&cost(1_001, 1_000));

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].limit, 1_000);
    }

    #[test]
    fn the_tolerance_allowance_rounds_down_and_saturates() {
        assert_eq!(allowed(1_000, 10), 1_100);
        assert_eq!(
            allowed(15, 10),
            16,
            "1.5 instructions of growth is not free"
        );
        assert_eq!(allowed(u64::MAX, 100), u64::MAX);
        assert_eq!(allowed(0, 100), 0, "a free operation stays free");
    }

    #[test]
    fn a_violation_renders_as_one_parseable_line() {
        let ceiling = BudgetGuard::new("transfer")
            .cpu_ceiling(1_000)
            .violations(&cost(1_500, 0))[0]
            .to_string();
        let growth = BudgetGuard::new("transfer")
            .baseline(Some(cost(1_000, 0)))
            .tolerance_percent(10)
            .violations(&cost(1_500, 0))[0]
            .to_string();

        assert_eq!(
            ceiling,
            "BUDGET kind=ceiling case=transfer metric=cpu_insns actual=1500 limit=1000"
        );
        assert_eq!(
            growth,
            "BUDGET kind=growth case=transfer metric=cpu_insns actual=1500 limit=1100 baseline=1000 tolerance_percent=10"
        );
    }

    #[test]
    #[should_panic(expected = "BUDGET kind=ceiling case=transfer metric=cpu_insns")]
    fn assert_within_panics_with_the_report() {
        BudgetGuard::new("transfer")
            .cpu_ceiling(10)
            .assert_within(&cost(20, 0));
    }

    #[test]
    fn a_baseline_file_round_trips_through_json() {
        let mut baseline = BudgetBaseline::new();
        baseline.record("transfer", cost(1_234, 56));
        baseline.record("mint", cost(99, 1));

        let text = baseline.to_json_string();
        let loaded = BudgetBaseline::parse(&text).unwrap();

        assert_eq!(loaded, baseline);
        assert_eq!(loaded.get("transfer"), Some(cost(1_234, 56)));
        assert_eq!(loaded.get("mint"), Some(cost(99, 1)));
        assert_eq!(loaded.len(), 2);
        assert!(!loaded.is_empty());
    }

    #[test]
    fn a_baseline_file_is_a_version_and_a_map_of_costs() {
        let mut baseline = BudgetBaseline::new();
        baseline.record("transfer", cost(1_234, 56));
        let text = baseline.to_json_string();

        assert!(text.contains(&format!("\"version\": {BASELINE_VERSION}")));
        assert!(text.contains("\"transfer\""));
        assert!(text.contains("\"cpu_insns\": 1234"));
    }

    #[test]
    fn a_baseline_from_a_newer_testkit_is_refused() {
        let text = r#"{ "version": 99, "cases": {} }"#;

        assert_eq!(
            BudgetBaseline::parse(text),
            Err(BaselineError::UnsupportedVersion { found: 99 })
        );
    }

    #[test]
    fn unreadable_baselines_fail_with_a_message_not_a_panic() {
        let malformed = BudgetBaseline::parse("not json at all");
        assert!(matches!(malformed, Err(BaselineError::Parse(_))));

        let missing = BudgetBaseline::load(std::path::Path::new("no-such-baseline.json"));
        assert!(matches!(missing, Err(BaselineError::Io(_))));
        assert!(missing
            .unwrap_err()
            .to_string()
            .starts_with("cannot read the baseline file"));
    }

    #[test]
    fn a_baseline_round_trips_through_disk() {
        let path = std::env::temp_dir().join(format!(
            "soroban-testkit-baseline-{}.json",
            std::process::id()
        ));
        let mut baseline = BudgetBaseline::new();
        baseline.record("transfer", cost(7, 11));

        baseline.save(&path).unwrap();
        let loaded = BudgetBaseline::load(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(loaded.get("transfer"), Some(cost(7, 11)));
    }

    #[test]
    fn a_guard_picked_from_a_file_carries_the_recorded_cost() {
        let mut baseline = BudgetBaseline::new();
        baseline.record("transfer", cost(1_000, 1_000));

        let violations = baseline
            .guard("transfer")
            .tolerance_percent(5)
            .violations(&cost(1_060, 1_000));
        assert_eq!(violations.len(), 1);

        assert!(baseline
            .guard("transfer")
            .tolerance_percent(5)
            .violations(&cost(1_050, 1_050))
            .is_empty());
        assert!(
            baseline
                .guard("unknown")
                .violations(&cost(u64::MAX, u64::MAX))
                .is_empty(),
            "an unrecorded case has no limits to break until a ceiling is added"
        );
    }

    #[test]
    fn cases_are_iterated_in_name_order() {
        let mut baseline = BudgetBaseline::new();
        baseline.record("mint", cost(1, 1));
        baseline.record("burn", cost(2, 2));

        let names: Vec<&str> = baseline.cases().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["burn", "mint"]);
    }

    #[contract]
    pub struct Meter;

    #[contractimpl]
    impl Meter {
        pub fn peek(env: Env) -> u32 {
            env.storage()
                .instance()
                .get(&Symbol::new(&env, "n"))
                .unwrap_or(0)
        }

        pub fn bump(env: Env) {
            let key = Symbol::new(&env, "n");
            let next: u32 = env.storage().instance().get(&key).unwrap_or(0) + 1;
            env.storage().instance().set(&key, &next);
        }
    }

    #[test]
    fn run_measures_a_real_invocation_and_lets_it_pass() {
        let env = soroban_sdk::Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.peek();

        BudgetGuard::new("bump")
            .cpu_ceiling(50_000_000)
            .mem_ceiling(50_000_000)
            .run(&env, || client.bump());
    }

    #[test]
    #[should_panic(expected = "BUDGET kind=ceiling case=bump metric=cpu_insns")]
    fn run_fails_the_test_when_the_invocation_breaches_its_ceiling() {
        let env = soroban_sdk::Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.peek();

        BudgetGuard::new("bump").cpu_ceiling(1).run(&env, || {
            client.bump();
        });
    }

    #[test]
    fn the_macro_measures_the_invocation_it_wraps() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.peek();

        budget_guard!(&env, "bump", { cpu_max: 50_000_000, mem_max: 50_000_000 }, || {
            client.bump();
        });
    }

    #[test]
    #[should_panic(expected = "BUDGET kind=ceiling case=bump metric=cpu_insns")]
    fn the_macro_reports_a_ceiling_breach_under_the_case_name() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.peek();

        budget_guard!(&env, "bump", { cpu_max: 1 }, || { client.bump(); });
    }

    #[test]
    fn the_macro_passes_a_cost_that_grew_within_the_baseline() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.peek();

        budget_guard!(
            &env,
            "bump",
            { baseline: Some(cost(u64::MAX, u64::MAX)), tolerance: 10 },
            || { client.bump(); }
        );
    }

    #[test]
    #[should_panic(expected = "BUDGET kind=growth case=bump metric=cpu_insns")]
    fn the_macro_rejects_growth_past_the_recorded_baseline() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.peek();

        budget_guard!(
            &env,
            "bump",
            { baseline: Some(cost(1, 1)), tolerance: 0 },
            || { client.bump(); }
        );
    }
}
