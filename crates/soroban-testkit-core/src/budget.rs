use serde::{Deserialize, Serialize};
use soroban_env_host::{budget::Budget, InvocationResources};
use std::collections::BTreeMap;

/// Anything Testkit can read consumed CPU and memory from.
///
/// Implemented for the host [`Budget`], for the SDK budget returned by
/// `env.cost_estimate().budget()`, and for the [`InvocationResources`] a call
/// leaves behind, so a snapshot can be captured from cumulative metering or
/// from one invocation.
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

// The metering is signed in the host and unsigned here; a negative reading is
// not something the runtime produces, so it reads as nothing consumed.
impl BudgetRead for InvocationResources {
    fn cpu_insns(&self) -> u64 {
        self.instructions.max(0) as u64
    }

    fn mem_bytes(&self) -> u64 {
        self.mem_bytes.max(0) as u64
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

    /// What the most recent top-level contract invocation metered.
    ///
    /// This is the cost of one call, not a running total, so it is the reading a
    /// test should assert on. The SDK reports it for the invocation that has
    /// already happened, so call this after the call you mean to measure; before
    /// any invocation it panics with the SDK's own message.
    ///
    /// A contract registered as a native test contract is metered without the
    /// VM's own costs — instantiation, execution and wasm reads are absent — so
    /// these numbers are lower than the same call would bill on a network.
    pub fn last_invocation(env: &soroban_sdk::Env) -> Self {
        Self::capture(&env.cost_estimate().resources())
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

    pub fn tolerance(mut self, percent: u64) -> Self {
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

// ---------------------------------------------------------------------------
// Comparing two baseline files
// ---------------------------------------------------------------------------

/// How one case differs between the base file and the head file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseChange {
    /// The head file records the case and the base file does not.
    Added,
    /// The base file records the case and the head file does not.
    Removed,
    /// Both files record the case and at least one metric moved.
    Changed,
    /// Both files record the case and both metrics are identical.
    Unchanged,
}

/// One case, as the base file and the head file each describe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetCaseDiff {
    case: String,
    base: Option<BudgetSnapshot>,
    head: Option<BudgetSnapshot>,
}

impl BudgetCaseDiff {
    pub fn case(&self) -> &str {
        &self.case
    }

    pub fn base(&self) -> Option<BudgetSnapshot> {
        self.base
    }

    pub fn head(&self) -> Option<BudgetSnapshot> {
        self.head
    }

    pub fn change(&self) -> CaseChange {
        match (self.base, self.head) {
            (None, Some(_)) => CaseChange::Added,
            (Some(_), None) => CaseChange::Removed,
            (Some(base), Some(head)) if base == head => CaseChange::Unchanged,
            _ => CaseChange::Changed,
        }
    }

    /// Instructions the case gained, or lost when negative.
    ///
    /// `None` when the case exists in only one of the two files: a case that was
    /// added or removed has no delta to report, only an absolute cost.
    pub fn cpu_delta(&self) -> Option<i128> {
        Some(delta(self.base?.cpu_insns, self.head?.cpu_insns))
    }

    /// Bytes the case gained, or lost when negative. Same rule as [`Self::cpu_delta`].
    pub fn mem_delta(&self) -> Option<i128> {
        Some(delta(self.base?.mem_bytes, self.head?.mem_bytes))
    }

    /// The CPU cost of the head run as a signed percentage of the base run.
    ///
    /// A base of zero grows by an unbounded percentage once the head records
    /// anything, which reads as `inf`; a case that stays at zero did not grow.
    pub fn cpu_growth_percent(&self) -> Option<f64> {
        growth(self.base?.cpu_insns, self.head?.cpu_insns)
    }

    pub fn mem_growth_percent(&self) -> Option<f64> {
        growth(self.base?.mem_bytes, self.head?.mem_bytes)
    }

    /// Whether either metric grew by more than `threshold_percent`.
    ///
    /// A case that is new has nothing to grow from, and a case that was removed
    /// cannot regress, so neither is a regression. Losing a case is still
    /// visible in the table — it is just not a reason to fail a build.
    pub fn exceeds(&self, threshold_percent: f64) -> bool {
        [self.cpu_growth_percent(), self.mem_growth_percent()]
            .into_iter()
            .flatten()
            .any(|percent| percent > threshold_percent)
    }
}

/// Every case of two baseline files, side by side.
///
/// This is what the `budget-diff` binary prints: the numbers a pull request
/// comment needs, and the verdict a CI step exits on. Cases are ordered by
/// name so two runs of the same pair of files produce the same report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetDiff {
    rows: Vec<BudgetCaseDiff>,
}

impl BudgetDiff {
    pub fn compare(base: &BudgetBaseline, head: &BudgetBaseline) -> Self {
        let mut names: Vec<&str> = base.cases().map(|(name, _)| name).collect();
        for (name, _) in head.cases() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort_unstable();

        let rows = names
            .into_iter()
            .map(|name| BudgetCaseDiff {
                case: name.to_string(),
                base: base.get(name),
                head: head.get(name),
            })
            .collect();
        Self { rows }
    }

    pub fn rows(&self) -> &[BudgetCaseDiff] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The cases that grew past `threshold_percent`, in report order.
    pub fn exceeding(&self, threshold_percent: f64) -> Vec<&BudgetCaseDiff> {
        self.rows
            .iter()
            .filter(|row| row.exceeds(threshold_percent))
            .collect()
    }

    /// A GitHub-flavoured Markdown report, ready to be posted as a PR comment.
    ///
    /// Every row carries an indicator: 🔴 for a case past the threshold, 🟢 for
    /// one that stayed inside it, and a note in the last column when a case is
    /// new or gone — those are not regressions, but a reviewer should see them.
    pub fn to_markdown(&self, threshold_percent: f64) -> String {
        let regressions = self.exceeding(threshold_percent);
        let mut report = String::new();
        report.push_str("### Soroban budget diff\n\n");
        report.push_str(&format!(
            "Threshold: `{}%`. {} case(s) compared, {} regression(s).\n\n",
            trim(threshold_percent),
            self.rows.len(),
            regressions.len()
        ));

        if self.rows.is_empty() {
            report.push_str("No case is recorded in either baseline file.\n");
            return report;
        }

        report.push_str("| Case | CPU instructions | Δ CPU | Memory bytes | Δ memory | |\n");
        report.push_str("|------|------------------|-------|--------------|----------|---|\n");
        for row in &self.rows {
            report.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                row.case,
                cell(
                    row.base.map(|cost| cost.cpu_insns),
                    row.head.map(|cost| cost.cpu_insns)
                ),
                delta_cell(row.cpu_delta(), row.cpu_growth_percent()),
                cell(
                    row.base.map(|cost| cost.mem_bytes),
                    row.head.map(|cost| cost.mem_bytes)
                ),
                delta_cell(row.mem_delta(), row.mem_growth_percent()),
                status_cell(row, threshold_percent),
            ));
        }

        if !regressions.is_empty() {
            report.push('\n');
            report.push_str(&format!(
                "**{}** case(s) grew past the {}% threshold.\n",
                regressions.len(),
                trim(threshold_percent)
            ));
        }
        report
    }

    /// One `BUDGET_DIFF` line per case for a job log or a step summary.
    pub fn to_summary_lines(&self, threshold_percent: f64) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        let mut lines = String::new();
        for row in &self.rows {
            let verdict = if row.exceeds(threshold_percent) {
                "regressed"
            } else {
                "ok"
            };
            lines.push_str(&format!(
                "BUDGET_DIFF case={} change={} cpu_base={} cpu_head={} cpu_drift={} mem_base={} mem_head={} mem_drift={} verdict={}\n",
                row.case,
                change_name(row.change()),
                optional(row.base.map(|cost| cost.cpu_insns)),
                optional(row.head.map(|cost| cost.cpu_insns)),
                percent_cell(row.cpu_growth_percent()),
                optional(row.base.map(|cost| cost.mem_bytes)),
                optional(row.head.map(|cost| cost.mem_bytes)),
                percent_cell(row.mem_growth_percent()),
                verdict,
            ));
        }
        lines
    }
}

/// Where the `budget-diff` binary writes its report and what it exits on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BudgetDiffCliOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

pub const BUDGET_DIFF_USAGE: &str = "\
usage: budget-diff BASE.json HEAD.json [options]

  --base FILE              baseline file (alternative to the first path)
  --head FILE              file measured on this run (alternative to the second path)
  --threshold PCT          how far a case may grow before it fails the run (default 0)
  --fail-on-regression PCT alias for --threshold
  --output-markdown FILE   also write the Markdown report to FILE
  --output FILE            alias for --output-markdown
  --allow-missing-base     treat a missing BASE file as an empty baseline
  -h, --help               print this message

exits 0 when no case grew past the threshold, 1 when one did, 2 on a usage error
";

/// Parse the `budget-diff` arguments and produce its report and exit code.
pub fn run_budget_diff_cli(args: impl IntoIterator<Item = String>) -> BudgetDiffCliOutcome {
    match budget_diff_cli(args.into_iter().collect()) {
        Ok(outcome) => outcome,
        Err(message) => BudgetDiffCliOutcome {
            stdout: String::new(),
            stderr: format!("budget-diff: {message}\n\n{BUDGET_DIFF_USAGE}"),
            exit_code: 2,
        },
    }
}

fn budget_diff_cli(args: Vec<String>) -> Result<BudgetDiffCliOutcome, String> {
    let mut paths = Vec::new();
    let mut base_path = None;
    let mut head_path = None;
    let mut threshold = 0.0f64;
    let mut output = None;
    let mut allow_missing_base = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(BudgetDiffCliOutcome {
                    stdout: BUDGET_DIFF_USAGE.to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
            "--base" => base_path = Some(args.next().ok_or("--base needs a file path")?),
            "--head" => head_path = Some(args.next().ok_or("--head needs a file path")?),
            "--threshold" | "--fail-on-regression" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{arg} needs a percentage"))?;
                threshold = parse_threshold(&value)?;
            }
            "--output-markdown" | "--output" => {
                output = Some(args.next().ok_or("--output-markdown needs a file path")?)
            }
            "--allow-missing-base" => allow_missing_base = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ => paths.push(arg),
        }
    }

    let base_path = match (base_path, paths.first()) {
        (Some(path), _) => path,
        (None, Some(path)) => path.clone(),
        (None, None) => return Err("missing the BASE baseline file".to_string()),
    };
    let head_path = match (head_path, paths.get(1)) {
        (Some(path), _) => path,
        (None, Some(path)) => path.clone(),
        (None, None) => return Err("missing the HEAD baseline file".to_string()),
    };

    let base_path = std::path::Path::new(&base_path);
    let base = if allow_missing_base && !base_path.exists() {
        BudgetBaseline::new()
    } else {
        BudgetBaseline::load(base_path)
            .map_err(|error| format!("{}: {error}", base_path.display()))?
    };
    let head = BudgetBaseline::load(std::path::Path::new(&head_path))
        .map_err(|error| format!("{head_path}: {error}"))?;

    let diff = BudgetDiff::compare(&base, &head);
    let markdown = diff.to_markdown(threshold);
    if let Some(output) = output {
        std::fs::write(&output, markdown.as_bytes())
            .map_err(|error| format!("cannot write {output}: {error}"))?;
    }

    let regressions = diff.exceeding(threshold).len();
    let mut stdout = markdown;
    stdout.push('\n');
    stdout.push_str(&diff.to_summary_lines(threshold));
    Ok(BudgetDiffCliOutcome {
        stdout,
        stderr: if regressions > 0 {
            format!(
                "budget-diff: {regressions} case(s) grew past the {}% threshold\n",
                trim(threshold)
            )
        } else {
            String::new()
        },
        exit_code: u8::from(regressions > 0),
    })
}

fn parse_threshold(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .trim_end_matches('%')
        .trim()
        .parse()
        .map_err(|_| format!("--threshold expects a number, got {value:?}"))?;
    if !parsed.is_finite() || parsed < 0.0 {
        return Err(format!(
            "--threshold must be a non-negative number, got {value:?}"
        ));
    }
    Ok(parsed)
}

fn delta(base: u64, head: u64) -> i128 {
    i128::from(head) - i128::from(base)
}

fn growth(base: u64, head: u64) -> Option<f64> {
    if base == 0 {
        return Some(if head == 0 { 0.0 } else { f64::INFINITY });
    }
    Some((i128::from(head) - i128::from(base)) as f64 / base as f64 * 100.0)
}

fn change_name(change: CaseChange) -> &'static str {
    match change {
        CaseChange::Added => "added",
        CaseChange::Removed => "removed",
        CaseChange::Changed => "changed",
        CaseChange::Unchanged => "unchanged",
    }
}

fn cell(base: Option<u64>, head: Option<u64>) -> String {
    match (base, head) {
        (Some(base), Some(head)) if base == head => head.to_string(),
        (base, head) => format!("{} → {}", optional(base), optional(head)),
    }
}

fn delta_cell(delta: Option<i128>, growth: Option<f64>) -> String {
    match (delta, growth) {
        (Some(0), Some(_)) => "0".to_string(),
        (Some(delta), Some(growth)) => signed(delta, growth),
        _ => "–".to_string(),
    }
}

fn status_cell(row: &BudgetCaseDiff, threshold: f64) -> String {
    match row.change() {
        CaseChange::Added => "🟢 new case".to_string(),
        CaseChange::Removed => "🟢 removed".to_string(),
        _ if row.exceeds(threshold) => "🔴".to_string(),
        _ => "🟢".to_string(),
    }
}

fn signed(delta: i128, growth: f64) -> String {
    let sign = if delta > 0 { "+" } else { "-" };
    format!("{sign}{} ({sign}{}%)", delta.abs(), trim(growth.abs()))
}

fn optional(value: Option<u64>) -> String {
    value.map_or_else(|| "–".to_string(), |value| value.to_string())
}

fn percent_cell(growth: Option<f64>) -> String {
    match growth {
        Some(growth) if growth.is_infinite() => "inf".to_string(),
        Some(growth) => format!("{}{}%", if growth > 0.0 { "+" } else { "" }, trim(growth)),
        None => "-".to_string(),
    }
}

/// `10`, `2.5` and `33.33` rather than `10.00`.
fn trim(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        allowed, run_budget_diff_cli, BaselineError, BudgetBaseline, BudgetDiff, BudgetGuard,
        BudgetMetric, BudgetSnapshot, CaseChange, ViolationKind, BASELINE_VERSION,
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
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("soroban-testkit-baseline-{id}-{nonce}.json"));
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
    fn the_last_invocation_reports_what_the_call_cost() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);

        client.bump();
        let write = BudgetSnapshot::last_invocation(&env);

        assert!(
            write.cpu_insns > 0 && write.mem_bytes > 0,
            "a contract call that writes must meter both resources"
        );
    }

    #[test]
    fn a_write_meters_more_than_a_read_of_the_same_entry() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);

        client.bump();
        let write = BudgetSnapshot::last_invocation(&env);
        client.peek();
        let read = BudgetSnapshot::last_invocation(&env);

        assert!(
            write.cpu_insns > read.cpu_insns,
            "a write ({}) and a read ({}) cannot cost the same",
            write.cpu_insns,
            read.cpu_insns
        );
    }

    #[test]
    fn two_identical_reads_measure_the_same() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.bump();

        client.peek();
        let first = BudgetSnapshot::last_invocation(&env);
        client.peek();
        let second = BudgetSnapshot::last_invocation(&env);

        assert_eq!(
            first, second,
            "a call that changes nothing has a cost that is a property of the call"
        );
    }

    #[test]
    fn capture_reads_invocation_resources_like_any_other_source() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);

        client.peek();

        assert_eq!(
            BudgetSnapshot::capture(&env.cost_estimate().resources()),
            BudgetSnapshot::last_invocation(&env)
        );
    }

    #[test]
    fn run_measures_a_real_invocation_and_lets_it_pass() {
        let env = soroban_sdk::Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);

        BudgetGuard::new("bump")
            .cpu_ceiling(50_000_000)
            .mem_ceiling(50_000_000)
            .run(&env, || client.bump());
    }

    #[test]
    fn run_hands_back_the_value_of_the_invocation_it_measured() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.bump();

        let seen = BudgetGuard::new("peek")
            .cpu_ceiling(1_000_000)
            .run(&env, || client.peek());

        assert_eq!(seen, 1);
    }

    #[test]
    #[should_panic(expected = "BUDGET kind=ceiling case=bump metric=cpu_insns")]
    fn run_fails_the_test_when_the_invocation_breaches_its_ceiling() {
        let env = soroban_sdk::Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);

        BudgetGuard::new("bump").cpu_ceiling(1).run(&env, || {
            client.bump();
        });
    }

    #[test]
    fn the_macro_measures_the_invocation_it_wraps() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);

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

        budget_guard!(&env, "bump", { cpu_max: 1 }, || { client.bump(); });
    }

    #[test]
    fn the_macro_passes_a_cost_that_grew_within_the_baseline() {
        let env = Env::default();
        let id = env.register(Meter, ());
        let client = MeterClient::new(&env, &id);
        client.bump();

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

        budget_guard!(
            &env,
            "bump",
            { baseline: Some(cost(1, 1)), tolerance: 0 },
            || { client.bump(); }
        );
    }

    fn snapshot(cpu_insns: u64, mem_bytes: u64) -> BudgetSnapshot {
        BudgetSnapshot {
            cpu_insns,
            mem_bytes,
        }
    }

    fn baseline(cases: &[(&str, u64, u64)]) -> BudgetBaseline {
        let mut baseline = BudgetBaseline::new();
        for (case, cpu, mem) in cases {
            baseline.record(*case, snapshot(*cpu, *mem));
        }
        baseline
    }

    fn diff_base_and_head() -> BudgetDiff {
        let base = baseline(&[("increment", 1_000, 500), ("gone", 40, 20)]);
        let head = baseline(&[
            ("increment", 1_500, 500),
            ("new_path", 900, 100),
            ("clean", 700, 300),
        ]);
        BudgetDiff::compare(&base, &head)
    }

    #[test]
    fn the_diff_orders_cases_and_names_every_change() {
        let diff = diff_base_and_head();
        let rows: Vec<(&str, CaseChange)> = diff
            .rows()
            .iter()
            .map(|row| (row.case(), row.change()))
            .collect();

        assert_eq!(
            rows,
            vec![
                ("clean", CaseChange::Added),
                ("gone", CaseChange::Removed),
                ("increment", CaseChange::Changed),
                ("new_path", CaseChange::Added),
            ]
        );
    }

    #[test]
    fn a_delta_is_signed_and_only_exists_on_both_sides() {
        let diff = diff_base_and_head();
        let increment = &diff.rows()[2];
        let gone = &diff.rows()[1];

        assert_eq!(increment.cpu_delta(), Some(500));
        assert_eq!(increment.mem_delta(), Some(0));
        assert_eq!(increment.cpu_growth_percent(), Some(50.0));
        assert_eq!(gone.cpu_delta(), None);
        assert_eq!(gone.cpu_growth_percent(), None);
    }

    #[test]
    fn growth_is_measured_against_a_zero_baseline_as_infinite() {
        let diff = BudgetDiff::compare(&baseline(&[("cold", 0, 0)]), &baseline(&[("cold", 10, 0)]));
        let row = &diff.rows()[0];

        assert_eq!(row.cpu_growth_percent(), Some(f64::INFINITY));
        assert_eq!(row.mem_growth_percent(), Some(0.0));
        assert!(row.exceeds(100.0));
    }

    #[test]
    fn only_growth_past_the_threshold_is_a_regression() {
        let diff = diff_base_and_head();

        assert!(diff.exceeding(10.0).is_empty() || diff.exceeding(10.0)[0].case() == "increment");
        assert_eq!(diff.exceeding(50.0).len(), 0, "50% is not past 50%");
        assert_eq!(diff.exceeding(49.9).len(), 1);
        assert_eq!(diff.exceeding(49.9)[0].case(), "increment");
    }

    #[test]
    fn the_markdown_report_marks_regressions_in_red() {
        let report = diff_base_and_head().to_markdown(10.0);

        assert!(report.starts_with("### Soroban budget diff"));
        assert!(report.contains("Threshold: `10%`"));
        assert!(report.contains("1 regression(s)"));
        assert!(report.contains("| `increment` | 1000 → 1500 | +500 (+50%) |"));
        assert!(report.contains("🔴"), "the grown case must be red");
        assert!(report.contains("🟢 new case"));
        assert!(report.contains("🟢 removed"));
        assert!(
            !report.contains("🔴 removed"),
            "a removed case is not a regression"
        );
    }

    #[test]
    fn a_clean_report_says_so_and_has_no_red_marker() {
        let diff = BudgetDiff::compare(&baseline(&[("a", 1, 2)]), &baseline(&[("a", 1, 2)]));
        let report = diff.to_markdown(0.0);

        assert!(report.contains("0 regression(s)"));
        assert!(report.contains("🟢"));
        assert!(!report.contains("🔴"));
        assert!(!report.contains("grew past"));
    }

    #[test]
    fn the_summary_lines_carry_the_verdict_for_ci() {
        let lines = diff_base_and_head().to_summary_lines(10.0);
        let increment = lines
            .lines()
            .find(|line| line.contains("case=increment"))
            .unwrap();

        assert!(increment.contains("change=changed"));
        assert!(increment.contains("cpu_drift=+50%"));
        assert!(increment.contains("mem_drift=0%"));
        assert!(increment.contains("verdict=regressed"));
        assert!(lines.contains("case=gone change=removed"));
        assert!(lines.contains("verdict=ok"));
    }

    fn temp_path(label: &str) -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("testkit-budget-diff-{id}-{label}-{nonce}.json"))
    }

    fn cli(args: &[&str]) -> super::BudgetDiffCliOutcome {
        run_budget_diff_cli(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn the_cli_fails_ci_past_the_threshold_and_writes_the_report() {
        let (base, head, report) = (temp_path("base"), temp_path("head"), temp_path("report"));
        baseline(&[("increment", 1_000, 500)]).save(&base).unwrap();
        baseline(&[("increment", 1_250, 500)]).save(&head).unwrap();
        let (b, h, r) = (
            base.to_str().unwrap(),
            head.to_str().unwrap(),
            report.to_str().unwrap(),
        );

        let pass = cli(&[b, h, "--threshold", "30"]);
        let fail = cli(&[b, h, "--fail-on-regression", "10%", "--output-markdown", r]);
        let written = std::fs::read_to_string(&report);
        for path in [&base, &head, &report] {
            let _ = std::fs::remove_file(path);
        }

        assert_eq!(pass.exit_code, 0, "{}", pass.stderr);
        assert!(pass.stdout.contains("**Pass**") || pass.stdout.contains("0 regression(s)"));
        assert_eq!(fail.exit_code, 1);
        assert!(fail
            .stderr
            .contains("1 case(s) grew past the 10% threshold"));
        assert_eq!(
            written.unwrap().trim_end(),
            fail.stdout
                .split("\n\nBUDGET_DIFF")
                .next()
                .unwrap()
                .trim_end()
        );
    }

    #[test]
    fn the_cli_accepts_the_named_base_and_head_flags() {
        let (base, head) = (temp_path("named-base"), temp_path("named-head"));
        baseline(&[("increment", 10, 10)]).save(&base).unwrap();
        baseline(&[("increment", 10, 10)]).save(&head).unwrap();
        let (b, h) = (base.to_str().unwrap(), head.to_str().unwrap());

        let outcome = cli(&["--base", b, "--head", h, "--threshold", "0"]);
        for path in [&base, &head] {
            let _ = std::fs::remove_file(path);
        }

        assert_eq!(outcome.exit_code, 0, "{}", outcome.stderr);
        assert!(outcome.stdout.contains("| `increment` | 10 | 0 | 10 | 0 |"));
    }

    #[test]
    fn the_cli_reports_usage_errors_with_status_2() {
        assert_eq!(cli(&[]).exit_code, 2);
        assert!(cli(&["only-one.json"])
            .stderr
            .contains("missing the HEAD baseline file"));
        assert!(cli(&["a", "b", "--threshold", "lots"])
            .stderr
            .contains("expects a number"));
        assert!(cli(&["a", "b", "--frobnicate"])
            .stderr
            .contains("unknown option --frobnicate"));
        assert!(cli(&["missing-base.json", "missing-head.json"])
            .stderr
            .contains("missing-base.json"));
        assert_eq!(cli(&["--help"]).exit_code, 0);
    }

    #[test]
    fn a_missing_base_can_be_allowed_on_the_first_run() {
        let head = temp_path("first-head");
        baseline(&[("increment", 500, 50)]).save(&head).unwrap();
        let h = head.to_str().unwrap();

        let refused = cli(&["does-not-exist.json", h]);
        let allowed = cli(&["does-not-exist.json", h, "--allow-missing-base"]);
        let _ = std::fs::remove_file(&head);

        assert_eq!(refused.exit_code, 2);
        assert_eq!(allowed.exit_code, 0, "{}", allowed.stderr);
        assert!(allowed.stdout.contains("🟢 new case"));
    }
}
