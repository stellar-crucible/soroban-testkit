use soroban_sdk::testutils::budget::ContractCostType;
use soroban_sdk::Env;

/// What one invocation did to host memory.
///
/// The host does not expose its object store, so every number here is read from
/// the metering the host charges while it runs: an allocation is a `MemAlloc`
/// charge, a copy is a `MemCpy` charge, and the live figure is the peak modelled
/// memory the invocation had in use. The entry counts come from the same
/// per-invocation resources [`crate::budget::BudgetSnapshot`] reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationProfile {
    /// The name the report and every failure message quote.
    pub case: String,
    /// Host allocations charged during the invocation — one per `MemAlloc`.
    pub allocations: u64,
    /// Bytes of host memory allocated by those charges, in total.
    pub allocated_bytes: u64,
    /// Peak modelled host memory in use during the invocation.
    ///
    /// This is the closest public reading to "objects live at the peak": an
    /// allocation that was freed before the call returned raised
    /// `allocated_bytes` without raising this.
    pub peak_live_bytes: u64,
    /// Bytes the invocation copied between host objects —
    /// the metered cost of cloning a `Vec`, `Map`, `Bytes` or value.
    pub copied_bytes: u64,
    /// CPU instructions the invocation metered.
    pub cpu_insns: u64,
    /// In-memory ledger entries the invocation read.
    pub entries_read: u32,
    /// Ledger entries the invocation wrote, so they outlive the call.
    pub entries_written: u32,
}

impl AllocationProfile {
    /// Bytes allocated that were not live at the peak — temporary waste.
    pub fn wasted_bytes(&self) -> u64 {
        self.allocated_bytes.saturating_sub(self.peak_live_bytes)
    }

    /// `allocated_bytes / peak_live_bytes`.
    ///
    /// A churn ratio of one means every byte allocated was still live at the
    /// peak; ten means the invocation allocated ten times what it needed to
    /// keep. An invocation that allocated nothing but kept nothing live reads
    /// as one — there is no churn to speak of.
    pub fn churn_ratio(&self) -> f64 {
        if self.peak_live_bytes == 0 {
            if self.allocated_bytes == 0 {
                1.0
            } else {
                f64::INFINITY
            }
        } else {
            self.allocated_bytes as f64 / self.peak_live_bytes as f64
        }
    }

    /// Memory efficiency, 0–100.
    ///
    /// The share of everything the invocation allocated that was still live at
    /// the peak. An invocation that allocated nothing scores 100: there was no
    /// waste to have. A read that allocates a large temporary and promptly
    /// drops it scores low even though it wrote nothing, which is the point —
    /// the pressure is in the churn, not in the storage it left behind.
    pub fn efficiency_score(&self) -> u8 {
        if self.allocated_bytes == 0 {
            return 100;
        }
        let live = self.peak_live_bytes.min(self.allocated_bytes);
        ((u128::from(live) * 100) / u128::from(self.allocated_bytes)) as u8
    }

    /// Whether this profile churns at least `min_churn_ratio` times its peak.
    pub fn is_hotspot(&self, min_churn_ratio: f64) -> bool {
        self.allocated_bytes > 0 && self.churn_ratio() >= min_churn_ratio
    }

    /// Human-readable byte count, e.g. `4.2 KB`. Powers the Markdown report.
    pub fn human_allocated(&self) -> String {
        human_bytes(self.allocated_bytes)
    }

    /// Human-readable peak live memory, e.g. `1.0 KB`.
    pub fn human_peak_live(&self) -> String {
        human_bytes(self.peak_live_bytes)
    }

    /// Human-readable copied bytes, e.g. `512 B`.
    pub fn human_copied(&self) -> String {
        human_bytes(self.copied_bytes)
    }
}

/// Every invocation the auditor measured, plus the Markdown view of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AllocationReport {
    profiles: Vec<AllocationProfile>,
}

impl AllocationReport {
    pub fn new(profiles: Vec<AllocationProfile>) -> Self {
        Self { profiles }
    }

    pub fn profiles(&self) -> &[AllocationProfile] {
        &self.profiles
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// The profiles that churned at least `min_churn_ratio` times their peak,
    /// in the order they were measured.
    pub fn hotspots(&self, min_churn_ratio: f64) -> Vec<&AllocationProfile> {
        self.profiles
            .iter()
            .filter(|profile| profile.is_hotspot(min_churn_ratio))
            .collect()
    }

    /// Fail unless every measured function stayed under `min_churn_ratio`.
    ///
    /// The panic names every hotspot in one message, so a suite that measures
    /// ten functions finds all of the wasteful ones in a single run.
    ///
    /// # Panics
    ///
    /// Panics when at least one profile churns `min_churn_ratio` times its peak
    /// live memory.
    pub fn assert_no_hotspots(&self, min_churn_ratio: f64) {
        let hotspots = self.hotspots(min_churn_ratio);
        if hotspots.is_empty() {
            return;
        }
        let lines = hotspots
            .iter()
            .map(|profile| {
                format!(
                    "MEMORY case={} churn={:.2} allocated_bytes={} peak_live_bytes={} efficiency={}",
                    profile.case,
                    profile.churn_ratio(),
                    profile.allocated_bytes,
                    profile.peak_live_bytes,
                    profile.efficiency_score(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "{} function(s) churned at least {min_churn_ratio:.2}x their peak live memory:\n{lines}",
            hotspots.len()
        );
    }

    /// The whole report as Markdown: one table row per invocation, then the
    /// hotspot and copy-pressure sections.
    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Memory pressure report\n\n");

        if self.profiles.is_empty() {
            out.push_str("No invocation was measured.\n");
            return out;
        }

        out.push_str(
            "| Case | Allocations | Allocated | Live (peak) | Churn | Copied | Entries read | Entries written | Efficiency |\n",
        );
        out.push_str(
            "|------|------------:|----------:|------------:|------:|-------:|-------------:|----------------:|-----------:|\n",
        );
        for profile in &self.profiles {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {:.2}x | {} | {} | {} | {}% |\n",
                profile.case,
                profile.allocations,
                profile.human_allocated(),
                profile.human_peak_live(),
                profile.churn_ratio(),
                profile.human_copied(),
                profile.entries_read,
                profile.entries_written,
                profile.efficiency_score(),
            ));
        }

        out.push_str("\n## Hotspots\n\n");
        let hotspots = self.hotspots(2.0);
        if hotspots.is_empty() {
            out.push_str("No function churned twice its peak live memory.\n");
        } else {
            let mut sorted = hotspots;
            sorted.sort_by(|a, b| {
                b.churn_ratio()
                    .total_cmp(&a.churn_ratio())
                    .then_with(|| a.case.cmp(&b.case))
            });
            for profile in sorted {
                out.push_str(&format!(
                    "- `{}` allocated {} to keep {} live ({:.2}x churn, {}% efficiency)\n",
                    profile.case,
                    profile.human_allocated(),
                    profile.human_peak_live(),
                    profile.churn_ratio(),
                    profile.efficiency_score(),
                ));
            }
        }

        out.push_str("\n## Copy pressure\n\n");
        let mut copied: Vec<&AllocationProfile> = self
            .profiles
            .iter()
            .filter(|profile| profile.copied_bytes > 0)
            .collect();
        if copied.is_empty() {
            out.push_str("No invocation copied host memory.\n");
        } else {
            copied.sort_by(|a, b| {
                b.copied_bytes
                    .cmp(&a.copied_bytes)
                    .then_with(|| a.case.cmp(&b.case))
            });
            for profile in copied {
                out.push_str(&format!(
                    "- `{}` copied {} across {} allocation(s)\n",
                    profile.case,
                    profile.human_copied(),
                    profile.allocations,
                ));
            }
        }

        out
    }
}

/// Measures the host memory each invocation spends, one function at a time.
///
/// ```rust,no_run
/// use soroban_testkit_core::memory::MemoryPressureAuditor;
///
/// fn main() {
///     let env = soroban_sdk::Env::default();
///     let mut auditor = MemoryPressureAuditor::new();
///     auditor.measure(&env, "transfer", || {});
///     println!("{}", auditor.to_markdown());
/// }
/// ```
///
/// The auditor rewinds the host's cost tracker before the closure and reads it
/// after, so each profile describes exactly the invocation it wrapped — not the
/// test setup that came before it. Two calls to the same function give two
/// profiles; measure the two cases separately instead of wrapping both in one
/// closure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryPressureAuditor {
    report: AllocationReport,
}

impl MemoryPressureAuditor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `invocation`, measure what it allocated, and return its value.
    ///
    /// The measurement is the metering the invocation left behind, so a closure
    /// that makes several contract calls is judged on all of them together.
    /// Wrap each call in its own `measure` to keep one profile per call.
    pub fn measure<T>(
        &mut self,
        env: &Env,
        case: impl Into<String>,
        invocation: impl FnOnce() -> T,
    ) -> T {
        self.reset_tracking(env);
        let value = invocation();
        self.record(env, case);
        value
    }

    /// Rewind the cost tracker so the next [`MemoryPressureAuditor::record`]
    /// describes only what happens after this call.
    ///
    /// [`MemoryPressureAuditor::measure`] calls this for you; it is public for a
    /// test that wants to drive the invocation itself.
    pub fn reset_tracking(&self, env: &Env) {
        env.cost_estimate().budget().reset_tracker();
    }

    /// Record the metering of the invocation that just ran.
    ///
    /// This does not reset the tracker — call [`MemoryPressureAuditor::reset_tracking`]
    /// before the invocation, or use [`MemoryPressureAuditor::measure`], which
    /// does both.
    ///
    /// # Panics
    ///
    /// Panics when `env` has not run a contract invocation yet: the host only
    /// keeps per-invocation resources for the last top-level call, and there is
    /// nothing to report before the first one.
    pub fn record(&mut self, env: &Env, case: impl Into<String>) {
        let budget = env.cost_estimate().budget();
        let allocation = budget.tracker(ContractCostType::MemAlloc);
        let copy = budget.tracker(ContractCostType::MemCpy);
        let resources = env.cost_estimate().resources();

        self.report.profiles.push(AllocationProfile {
            case: case.into(),
            allocations: allocation.iterations,
            allocated_bytes: allocation.inputs.unwrap_or(0),
            peak_live_bytes: budget.memory_bytes_cost(),
            copied_bytes: copy.inputs.unwrap_or(0),
            cpu_insns: budget.cpu_instruction_cost(),
            entries_read: resources.memory_read_entries,
            entries_written: resources.write_entries,
        });
    }

    pub fn profiles(&self) -> &[AllocationProfile] {
        self.report.profiles()
    }

    pub fn report(&self) -> AllocationReport {
        self.report.clone()
    }

    pub fn to_markdown(&self) -> String {
        self.report.to_markdown()
    }

    pub fn len(&self) -> usize {
        self.report.len()
    }

    pub fn is_empty(&self) -> bool {
        self.report.is_empty()
    }

    /// Forget every profile measured so far.
    pub fn clear(&mut self) {
        self.report = AllocationReport::default();
    }
}

/// Render a byte count the way the report writes it: `983 B`, `4.2 KB`, `1.1 MB`.
///
/// One decimal place, and no trailing `.0` below a kilobyte, so a table of
/// readings stays aligned and a diff of two runs reads as a number changing.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;

    let value = bytes as f64;
    if value < KB {
        format!("{bytes} B")
    } else if value < MB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{:.1} MB", value / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::{human_bytes, AllocationProfile, AllocationReport, MemoryPressureAuditor};
    use soroban_sdk::{contract, contractimpl, symbol_short, Env};

    #[contract]
    pub struct Allocator;

    #[contractimpl]
    impl Allocator {
        /// One small value, written once: the efficient baseline.
        pub fn store_one(env: Env) -> u32 {
            env.storage().persistent().set(&symbol_short!("one"), &1u32);
            1
        }

        /// Builds a large temporary vector, copies it, and keeps one number.
        pub fn churn(env: Env) {
            let mut values: soroban_sdk::Vec<u32> = soroban_sdk::Vec::new(&env);
            for index in 0..512u32 {
                values.push_back(index);
            }
            let copy = values.clone();
            env.storage()
                .persistent()
                .set(&symbol_short!("last"), &copy.len());
        }
    }

    fn setup() -> (Env, soroban_sdk::Address) {
        let env = Env::default();
        let id = env.register(Allocator, ());
        (env, id)
    }

    fn profile(case: &str, allocated_bytes: u64, peak_live_bytes: u64) -> AllocationProfile {
        AllocationProfile {
            case: case.to_string(),
            allocations: 1,
            allocated_bytes,
            peak_live_bytes,
            copied_bytes: 0,
            cpu_insns: 0,
            entries_read: 0,
            entries_written: 0,
        }
    }

    #[test]
    fn measure_hands_back_the_value_of_the_invocation() {
        let (env, id) = setup();
        let client = AllocatorClient::new(&env, &id);
        let mut auditor = MemoryPressureAuditor::new();

        let seen = auditor.measure(&env, "store_one", || client.store_one());

        assert_eq!(seen, 1);
        assert_eq!(auditor.len(), 1);
        assert_eq!(auditor.profiles()[0].case, "store_one");
    }

    #[test]
    fn measure_records_the_allocations_the_invocation_charged() {
        let (env, id) = setup();
        let client = AllocatorClient::new(&env, &id);
        let mut auditor = MemoryPressureAuditor::new();

        auditor.measure(&env, "churn", || client.churn());

        let churn = &auditor.profiles()[0];
        assert!(
            churn.allocations > 0,
            "building a 512-element vector must allocate host memory"
        );
        assert!(churn.allocated_bytes > 0);
        assert!(churn.peak_live_bytes > 0, "the invocation kept memory live");
        assert!(churn.entries_written > 0, "the call wrote one entry");
    }

    #[test]
    fn a_function_that_churns_allocates_more_than_one_that_stores_and_stops() {
        let (env, id) = setup();
        let client = AllocatorClient::new(&env, &id);
        let mut auditor = MemoryPressureAuditor::new();

        auditor.measure(&env, "store_one", || client.store_one());
        auditor.measure(&env, "churn", || client.churn());

        let allocated: Vec<u64> = auditor
            .profiles()
            .iter()
            .map(|profile| profile.allocated_bytes)
            .collect();
        assert!(
            allocated[1] > allocated[0],
            "churn allocated {:?} and store_one allocated {:?}",
            allocated[1],
            allocated[0]
        );
    }

    #[test]
    fn the_tracker_is_rewound_so_profiles_do_not_accumulate() {
        let (env, id) = setup();
        let client = AllocatorClient::new(&env, &id);
        let mut auditor = MemoryPressureAuditor::new();

        // The first invocation of a contract pays the host's one-time costs for
        // reaching it; after that an identical call meters identically. A
        // tracker that kept accumulating would make every profile a sum of the
        // calls before it instead.
        auditor.measure(&env, "first", || client.store_one());
        auditor.measure(&env, "second", || client.store_one());
        auditor.measure(&env, "third", || client.store_one());

        let profiles = auditor.profiles();
        assert_eq!(
            profiles[1].allocations, profiles[2].allocations,
            "an identical call measured twice cannot allocate a different amount"
        );
        assert_eq!(profiles[1].allocated_bytes, profiles[2].allocated_bytes);
        assert!(
            profiles[2].allocations < profiles[0].allocations.saturating_mul(2),
            "the third profile ({}) is not the sum of the three calls ({} each)",
            profiles[2].allocations,
            profiles[0].allocations
        );
    }

    #[test]
    fn the_report_keeps_profiles_in_the_order_they_were_measured() {
        let (env, id) = setup();
        let client = AllocatorClient::new(&env, &id);
        let mut auditor = MemoryPressureAuditor::new();

        auditor.measure(&env, "churn", || client.churn());
        auditor.measure(&env, "store_one", || client.store_one());

        let cases: Vec<&str> = auditor
            .profiles()
            .iter()
            .map(|profile| profile.case.as_str())
            .collect();
        assert_eq!(cases, vec!["churn", "store_one"]);
    }

    #[test]
    fn clearing_forgets_every_profile() {
        let (env, id) = setup();
        let client = AllocatorClient::new(&env, &id);
        let mut auditor = MemoryPressureAuditor::new();
        auditor.measure(&env, "store_one", || client.store_one());

        auditor.clear();

        assert!(auditor.is_empty());
        assert!(auditor
            .to_markdown()
            .contains("No invocation was measured."));
    }

    #[test]
    fn a_profile_with_no_allocations_is_perfectly_efficient() {
        let profile = profile("noop", 0, 0);
        assert_eq!(profile.efficiency_score(), 100);
        assert_eq!(profile.churn_ratio(), 1.0);
        assert_eq!(profile.wasted_bytes(), 0);
        assert!(!profile.is_hotspot(2.0));
    }

    #[test]
    fn efficiency_is_the_share_of_allocated_bytes_still_live_at_the_peak() {
        assert_eq!(profile("half", 1_000, 500).efficiency_score(), 50);
        assert_eq!(profile("tenth", 1_000, 100).efficiency_score(), 10);
        assert_eq!(profile("all", 1_000, 1_000).efficiency_score(), 100);
        assert_eq!(
            profile("over", 1_000, 4_000).efficiency_score(),
            100,
            "a peak larger than the allocation volume is capped, not amplified"
        );
    }

    #[test]
    fn churn_ratio_and_waste_follow_the_allocated_and_peak_readings() {
        let profile = profile("churn", 4_096, 1_024);

        assert_eq!(profile.wasted_bytes(), 3_072);
        assert_eq!(profile.churn_ratio(), 4.0);
        assert!(profile.is_hotspot(2.0));
        assert!(!profile.is_hotspot(5.0));
    }

    #[test]
    fn an_allocation_with_no_live_peak_is_infinite_churn() {
        let profile = profile("leak", 1_024, 0);

        assert!(profile.churn_ratio().is_infinite());
        assert_eq!(profile.efficiency_score(), 0);
        assert!(profile.is_hotspot(1_000.0));
    }

    #[test]
    fn hotspots_are_filtered_by_the_ratio_the_caller_asks_for() {
        let report = AllocationReport::new(vec![
            profile("clean", 1_000, 1_000),
            profile("warm", 1_500, 1_000),
            profile("hot", 5_000, 1_000),
        ]);

        let names = |profiles: Vec<&AllocationProfile>| {
            profiles
                .into_iter()
                .map(|profile| profile.case.clone())
                .collect::<Vec<_>>()
        };

        assert_eq!(names(report.hotspots(2.0)), vec!["hot"]);
        assert_eq!(names(report.hotspots(1.5)), vec!["warm", "hot"]);
        assert!(report.hotspots(10.0).is_empty());
    }

    #[test]
    fn the_markdown_report_has_a_table_row_per_case_and_the_sections() {
        let report = AllocationReport::new(vec![
            AllocationProfile {
                case: "transfer".to_string(),
                allocations: 12,
                allocated_bytes: 4_096,
                peak_live_bytes: 1_024,
                copied_bytes: 512,
                cpu_insns: 90_000,
                entries_read: 3,
                entries_written: 2,
            },
            profile("mint", 0, 0),
        ]);

        let markdown = report.to_markdown();
        assert!(markdown.starts_with("# Memory pressure report"));
        assert!(markdown.contains("| Case | Allocations | Allocated | Live (peak) |"));
        assert!(markdown
            .contains("| `transfer` | 12 | 4.0 KB | 1.0 KB | 4.00x | 512 B | 3 | 2 | 25% |"));
        assert!(markdown.contains("| `mint` | 1 | 0 B | 0 B | 1.00x | 0 B | 0 | 0 | 100% |"));
        assert!(markdown.contains(
            "- `transfer` allocated 4.0 KB to keep 1.0 KB live (4.00x churn, 25% efficiency)"
        ));
        assert!(markdown.contains("- `transfer` copied 512 B across 12 allocation(s)"));
    }

    #[test]
    fn the_markdown_report_says_so_when_nothing_was_wasteful() {
        let report = AllocationReport::new(vec![profile("clean", 1_000, 1_000)]);
        let markdown = report.to_markdown();

        assert!(markdown.contains("No function churned twice its peak live memory."));
        assert!(markdown.contains("No invocation copied host memory."));
    }

    #[test]
    fn hotspots_are_rendered_worst_first() {
        let report = AllocationReport::new(vec![
            profile("mild", 3_000, 1_000),
            profile("severe", 9_000, 1_000),
            profile("moderate", 5_000, 1_000),
        ]);

        let markdown = report.to_markdown();
        let hotspots = markdown
            .split("## Hotspots")
            .nth(1)
            .expect("the report has a hotspot section");
        let severe = hotspots.find("`severe`").unwrap();
        let moderate = hotspots.find("`moderate`").unwrap();
        let mild = hotspots.find("`mild`").unwrap();
        assert!(severe < moderate && moderate < mild);
    }

    #[test]
    #[should_panic(
        expected = "MEMORY case=hot churn=5.00 allocated_bytes=5000 peak_live_bytes=1000 efficiency=20"
    )]
    fn assert_no_hotspots_names_every_breach_in_one_panic() {
        AllocationReport::new(vec![profile("hot", 5_000, 1_000)]).assert_no_hotspots(2.0);
    }

    #[test]
    fn assert_no_hotspots_accepts_a_report_inside_the_limit() {
        AllocationReport::new(vec![profile("warm", 1_500, 1_000)]).assert_no_hotspots(2.0);
    }

    #[test]
    fn byte_counts_read_the_same_way_the_table_renders_them() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(983), "983 B");
        assert_eq!(human_bytes(1_024), "1.0 KB");
        assert_eq!(human_bytes(4_300), "4.2 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MB");
    }
}
