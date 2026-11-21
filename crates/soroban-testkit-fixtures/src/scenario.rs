//! Scripted multi-block scenarios over a [`TestContext`].
//!
//! Contracts that read `env.ledger().sequence()` or `timestamp()` — payment
//! streams, vesting lockups, bridge relays — fail in ways a single-call test
//! cannot see: a missed ledger, a stalled network, a rollback, arithmetic that
//! underflows when the clock moves backwards. [`ScenarioBuilder`] scripts those
//! conditions as a chain of steps, block advances, halts and rollbacks over one
//! context, then reports a readable timeline when any step fails.
//!
//! ```text
//! let mut scenario = ScenarioBuilder::new(&ctx);
//! scenario
//!     .step("Open stream", |ctx| stream_client.open(&1_000))
//!     .advance_blocks(100)
//!     .step("Claim partial", |ctx| stream_client.claim())
//!     .halt(3_600)
//!     .advance_blocks(500)
//!     .step("Final claim", |ctx| stream_client.claim());
//!
//! scenario.assert_all_passed();
//! ```
//!
//! Steps run through `catch_unwind`, so a contract panic does not abort the
//! scenario before the remaining timeline is recorded — [`ScenarioBuilder::assert_all_passed`]
//! then fails with the whole sequence, marking the step that panicked.
//!
//! [`TestContext`]: crate::TestContext

use std::any::Any;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::Env;

use crate::TestContext;

/// What happened at one point on a scenario's timeline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// The scenario opened here.
    Start,
    /// The step ran to completion.
    Passed,
    /// The step panicked or a check returned false.
    Failed(String),
}

impl fmt::Display for StepOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StepOutcome::Start => write!(formatter, "start"),
            StepOutcome::Passed => write!(formatter, "passed"),
            StepOutcome::Failed(reason) => write!(formatter, "FAILED: {reason}"),
        }
    }
}

/// One recorded point on the scenario timeline.
///
/// `sequence` and `timestamp` are the ledger values *after* the step, so a
/// failure can be read as the chain state the contract responded to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimelineEntry {
    pub index: usize,
    pub sequence: u32,
    pub timestamp: u64,
    pub label: String,
    pub detail: String,
    pub outcome: StepOutcome,
}

impl fmt::Display for TimelineEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "#{:02} seq={} t={} {}: {} ({})",
            self.index, self.sequence, self.timestamp, self.label, self.detail, self.outcome
        )
    }
}

/// A failure recorded during a scenario, pointing at the timeline entry that
/// produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScenarioFailure {
    pub entry_index: usize,
    pub label: String,
    pub message: String,
}

impl fmt::Display for ScenarioFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "scenario step #{} {:?}: {}",
            self.entry_index, self.label, self.message
        )
    }
}

/// Orchestrates a scripted sequence of contract calls and ledger movement over
/// one [`TestContext`].
///
/// The builder keeps a clone of the context's environment, so ledger changes it
/// makes are visible to every client the scenario's closures use. Methods chain:
/// `step`, `advance_blocks`, `advance_time`, `advance`, `halt`, `rollback_blocks`
/// and `set_clock` all return `&mut Self`.
pub struct ScenarioBuilder<'a> {
    ctx: &'a TestContext,
    env: Env,
    start_sequence: u32,
    start_timestamp: u64,
    timeline: Vec<TimelineEntry>,
    failures: Vec<ScenarioFailure>,
    last_probe: Option<(String, u64)>,
}

impl<'a> ScenarioBuilder<'a> {
    /// Start a scenario at the context's current ledger position.
    pub fn new(ctx: &'a TestContext) -> Self {
        let env = ctx.env.clone();
        let start_sequence = env.ledger().sequence();
        let start_timestamp = env.ledger().timestamp();
        let mut scenario = Self {
            ctx,
            env,
            start_sequence,
            start_timestamp,
            timeline: Vec::new(),
            failures: Vec::new(),
            last_probe: None,
        };
        scenario.push(
            "start",
            format!("seq={start_sequence} t={start_timestamp}"),
            StepOutcome::Start,
        );
        scenario
    }

    /// Run one contract invocation as a named step.
    ///
    /// A panic is caught and recorded rather than unwinding, so the rest of the
    /// script still runs and [`Self::assert_all_passed`] can print what came
    /// before and after the failure.
    pub fn step<F, R>(&mut self, label: impl Into<String>, invocation: F) -> &mut Self
    where
        F: FnOnce(&TestContext) -> R,
    {
        let label = label.into();
        let before = self.clock();
        let ctx = self.ctx;
        let outcome = catch_unwind(AssertUnwindSafe(move || {
            invocation(ctx);
        }));

        let after = self.clock();
        let detail = format!("seq {}->{}, t {}->{}", before.0, after.0, before.1, after.1);
        match outcome {
            Ok(()) => self.push(label, detail, StepOutcome::Passed),
            Err(payload) => {
                let message = panic_message(payload);
                self.fail(label, detail, message);
            }
        }
        self
    }

    /// Run a state-consistency check as a named step.
    ///
    /// The predicate receives the context, so a check can read a contract's
    /// state. A `false` result is a failure with the step's label; a panic is
    /// recorded the same way an invocation's panic is.
    pub fn check<F>(&mut self, label: impl Into<String>, predicate: F) -> &mut Self
    where
        F: FnOnce(&TestContext) -> bool,
    {
        let label = label.into();
        let before = self.clock();
        let ctx = self.ctx;
        let outcome = catch_unwind(AssertUnwindSafe(move || predicate(ctx)));
        let after = self.clock();
        let detail = format!("seq {}->{}, t {}->{}", before.0, after.0, before.1, after.1);

        match outcome {
            Ok(true) => self.push(label, detail, StepOutcome::Passed),
            Ok(false) => self.fail(
                label,
                detail,
                "consistency check returned false".to_string(),
            ),
            Err(payload) => self.fail(label, detail, panic_message(payload)),
        }
        self
    }

    /// Read a value from the contract and check that it never moves backwards.
    ///
    /// This is the clock-inversion check for contract logic: a vesting or
    /// streaming contract reports the time it believes it is at, and a value
    /// below the previous probe means a ledger rollback or an arithmetic
    /// underflow has made the contract see time run backwards. The failure
    /// message names both readings and the subtraction that would underflow.
    pub fn probe<F>(&mut self, label: impl Into<String>, observation: F) -> &mut Self
    where
        F: FnOnce(&TestContext) -> u64,
    {
        let label = label.into();
        let before = self.clock();
        let ctx = self.ctx;
        let outcome = catch_unwind(AssertUnwindSafe(move || observation(ctx)));
        let after = self.clock();
        let detail = format!("seq {}->{}, t {}->{}", before.0, after.0, before.1, after.1);

        match outcome {
            Ok(value) => {
                let inversion = match &self.last_probe {
                    Some((previous_label, previous)) if value < *previous => Some(format!(
                        "clock inversion in {label:?}: {value} after {previous_label:?} reported {previous} (a `{previous} - {value}` subtraction underflows)"
                    )),
                    _ => None,
                };
                self.last_probe = Some((label.clone(), value));
                match inversion {
                    Some(message) => self.fail(label, format!("{detail}, value={value}"), message),
                    None => self.push(
                        label,
                        format!("{detail}, value={value}"),
                        StepOutcome::Passed,
                    ),
                }
            }
            Err(payload) => self.fail(
                label,
                format!("{detail}, value unavailable"),
                panic_message(payload),
            ),
        }
        self
    }

    /// Advance `count` ledger closes, leaving the clock where it was.
    ///
    /// The increments are arbitrary: call it with 1, 100 and 500 in one scenario
    /// to model irregular block production. Saturates at `u32::MAX`.
    pub fn advance_blocks(&mut self, count: u32) -> &mut Self {
        let from = self.env.ledger().sequence();
        let to = from.saturating_add(count);
        self.env.ledger().set_sequence_number(to);
        self.push(
            format!("advance {count} block(s)"),
            format!("seq {from}->{to}"),
            StepOutcome::Passed,
        );
        self
    }

    /// Advance the unix clock by `seconds`, leaving the sequence where it was.
    ///
    /// Saturates at `u64::MAX`.
    pub fn advance_time(&mut self, seconds: u64) -> &mut Self {
        let from = self.env.ledger().timestamp();
        let to = from.saturating_add(seconds);
        self.env.ledger().set_timestamp(to);
        self.push(
            format!("advance {seconds}s"),
            format!("t {from}->{to}"),
            StepOutcome::Passed,
        );
        self
    }

    /// Advance both the sequence and the clock in one step.
    pub fn advance(&mut self, blocks: u32, seconds: u64) -> &mut Self {
        self.advance_blocks(blocks).advance_time(seconds)
    }

    /// Simulate a network halt of `seconds`: time passes, no block closes.
    ///
    /// A payment stream keeps accruing during the halt because its clock moves,
    /// while a contract that measures progress in ledger heights sees nothing
    /// happen — the difference is the bug this method exists to expose.
    pub fn halt(&mut self, seconds: u64) -> &mut Self {
        let from = self.env.ledger().timestamp();
        let to = from.saturating_add(seconds);
        self.env.ledger().set_timestamp(to);
        self.push(
            format!("halt {seconds}s"),
            format!("t {from}->{to}, seq={}", self.env.ledger().sequence()),
            StepOutcome::Passed,
        );
        self
    }

    /// Simulate a reorg that rewinds the ledger sequence by `count` closes.
    ///
    /// The clock is left alone, matching a chain that replaced a run of blocks
    /// with a shorter one. Saturates at zero. Unlike a probe inversion, a
    /// deliberate rollback is not itself a failure — the point is to see
    /// whether the contract copes with it.
    pub fn rollback_blocks(&mut self, count: u32) -> &mut Self {
        let from = self.env.ledger().sequence();
        let to = from.saturating_sub(count);
        self.env.ledger().set_sequence_number(to);
        self.push(
            format!("rollback {count} block(s)"),
            format!("seq {from}->{to}"),
            StepOutcome::Passed,
        );
        self
    }

    /// Jump to an explicit ledger position, forwards or backwards.
    ///
    /// This is the escape hatch for a reorg that changes both values at once,
    /// and for a test that wants to place the chain on a known date. A backwards
    /// jump is recorded on the timeline; a contract that then reports time
    /// moving backwards is caught by [`Self::probe`].
    pub fn set_clock(&mut self, sequence: u32, timestamp: u64) -> &mut Self {
        let before = self.clock();
        self.env.ledger().set_sequence_number(sequence);
        self.env.ledger().set_timestamp(timestamp);
        self.push(
            "set clock",
            format!(
                "seq {}->{}, t {}->{}",
                before.0, sequence, before.1, timestamp
            ),
            StepOutcome::Passed,
        );
        self
    }

    /// Every recorded timeline entry, in execution order.
    pub fn timeline(&self) -> &[TimelineEntry] {
        &self.timeline
    }

    /// Every failure the scenario recorded.
    pub fn failures(&self) -> &[ScenarioFailure] {
        &self.failures
    }

    /// The number of steps and ledger movements recorded.
    pub fn len(&self) -> usize {
        self.timeline.len()
    }

    /// Whether the timeline holds nothing but the opening entry.
    pub fn is_empty(&self) -> bool {
        self.timeline.len() <= 1
    }

    /// The sequence the scenario started at.
    pub fn start_sequence(&self) -> u32 {
        self.start_sequence
    }

    /// The timestamp the scenario started at.
    pub fn start_timestamp(&self) -> u64 {
        self.start_timestamp
    }

    /// Blocks closed since the scenario started.
    ///
    /// Uses `saturating_sub`, so a rollback below the starting height reports
    /// zero rather than wrapping.
    pub fn blocks_elapsed(&self) -> u32 {
        self.env
            .ledger()
            .sequence()
            .saturating_sub(self.start_sequence)
    }

    /// Seconds elapsed on the clock since the scenario started.
    ///
    /// A clock that has moved backwards reports zero rather than an underflowed
    /// number, which keeps the summary usable on a reorg timeline.
    pub fn seconds_elapsed(&self) -> u64 {
        self.env
            .ledger()
            .timestamp()
            .saturating_sub(self.start_timestamp)
    }

    /// The readable timeline, including every failure.
    pub fn format_timeline(&self) -> String {
        let mut lines = Vec::with_capacity(self.timeline.len() + self.failures.len() + 2);
        if self.failures.is_empty() {
            lines.push("SCENARIO PASSED".to_string());
        } else {
            lines.push(format!(
                "SCENARIO FAILED ({} failure(s))",
                self.failures.len()
            ));
        }
        lines.extend(self.timeline.iter().map(ToString::to_string));
        for failure in &self.failures {
            lines.push(format!("  -> {failure}"));
        }
        lines.join("\n")
    }

    /// Fail unless every step passed.
    ///
    /// The panic message is the full timeline, so a CI log shows the block
    /// heights and timestamps around the failing step rather than only the
    /// panic that came out of it.
    pub fn assert_all_passed(&self) {
        if !self.failures.is_empty() {
            panic!(
                "scenario failed with {} failure(s):\n{}",
                self.failures.len(),
                self.format_timeline()
            );
        }
    }

    /// The current `(sequence, timestamp)`.
    fn clock(&self) -> (u32, u64) {
        (self.env.ledger().sequence(), self.env.ledger().timestamp())
    }

    fn push(&mut self, label: impl Into<String>, detail: impl Into<String>, outcome: StepOutcome) {
        self.timeline.push(TimelineEntry {
            index: self.timeline.len(),
            sequence: self.env.ledger().sequence(),
            timestamp: self.env.ledger().timestamp(),
            label: label.into(),
            detail: detail.into(),
            outcome,
        });
    }

    /// Record a failed entry and the failure that produced it.
    fn fail(&mut self, label: String, detail: impl Into<String>, message: String) {
        let index = self.timeline.len();
        self.timeline.push(TimelineEntry {
            index,
            sequence: self.env.ledger().sequence(),
            timestamp: self.env.ledger().timestamp(),
            label: label.clone(),
            detail: detail.into(),
            outcome: StepOutcome::Failed(message.clone()),
        });
        self.failures.push(ScenarioFailure {
            entry_index: index,
            label,
            message,
        });
    }
}

/// Render a caught panic payload as text.
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{ScenarioBuilder, StepOutcome};
    use crate::builder::TestContextBuilder;
    use crate::TestContext;
    use soroban_sdk::{
        contract, contractimpl, contracttype, symbol_short, testutils::Ledger as _, Env,
    };

    /// A linear vesting position: nothing before `start`, everything after
    /// `duration`, and `total * elapsed / duration` in between.
    #[contracttype]
    #[derive(Clone, Debug, PartialEq)]
    pub struct VestingTerms {
        pub start: u64,
        pub duration: u64,
        pub total: i128,
    }

    #[contract]
    pub struct Vesting;

    #[contractimpl]
    impl Vesting {
        pub fn open(env: Env, start: u64, duration: u64, total: i128) {
            env.storage().persistent().set(
                &symbol_short!("terms"),
                &VestingTerms {
                    start,
                    duration,
                    total,
                },
            );
        }

        pub fn vested(env: Env) -> i128 {
            let terms: VestingTerms = env
                .storage()
                .persistent()
                .get(&symbol_short!("terms"))
                .expect("vesting position is open");
            let now = env.ledger().timestamp();
            if now <= terms.start {
                return 0;
            }
            let elapsed = now - terms.start;
            if elapsed >= terms.duration {
                terms.total
            } else {
                terms.total * elapsed as i128 / terms.duration as i128
            }
        }

        pub fn clock(env: Env) -> u64 {
            env.ledger().timestamp()
        }
    }

    /// A per-second payment stream that subtracts the open time from the clock.
    ///
    /// The subtraction is deliberate: if the clock ever moves backwards this
    /// contract panics with an arithmetic underflow, which is the bug a scenario
    /// is supposed to expose.
    #[contract]
    pub struct Stream;

    #[contractimpl]
    impl Stream {
        pub fn open(env: Env, start: u64, rate: i128) {
            env.storage()
                .persistent()
                .set(&symbol_short!("start"), &start);
            env.storage()
                .persistent()
                .set(&symbol_short!("rate"), &rate);
        }

        pub fn owed(env: Env) -> i128 {
            let start: u64 = env
                .storage()
                .persistent()
                .get(&symbol_short!("start"))
                .unwrap();
            let rate: i128 = env
                .storage()
                .persistent()
                .get(&symbol_short!("rate"))
                .unwrap();
            let elapsed = env.ledger().timestamp() - start;
            rate * elapsed as i128
        }

        pub fn clock(env: Env) -> u64 {
            env.ledger().timestamp()
        }
    }

    fn context() -> TestContext {
        TestContextBuilder::new().with_users(1).build()
    }

    fn vesting(ctx: &TestContext) -> (soroban_sdk::Address, VestingClient<'_>) {
        let id = ctx.env.register(Vesting, ());
        (id.clone(), VestingClient::new(&ctx.env, &id))
    }

    fn stream(ctx: &TestContext) -> (soroban_sdk::Address, StreamClient<'_>) {
        let id = ctx.env.register(Stream, ());
        (id.clone(), StreamClient::new(&ctx.env, &id))
    }

    #[test]
    fn a_scenario_records_steps_and_irregular_block_advances() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = vesting(&ctx);

        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| client.open(&1_700_000_000, &1_000, &10_000))
            .advance_blocks(1)
            .advance_blocks(100)
            .advance_time(500)
            .step("read", |_| client.vested());

        scenario.assert_all_passed();
        assert_eq!(scenario.failures().len(), 0);
        assert_eq!(scenario.timeline().len(), 6);
        assert_eq!(scenario.timeline()[0].outcome, StepOutcome::Start);
        assert_eq!(scenario.blocks_elapsed(), 101);
        assert_eq!(scenario.seconds_elapsed(), 500);
        assert_eq!(scenario.timeline().last().unwrap().sequence, 101);
        assert_eq!(scenario.timeline().last().unwrap().timestamp, 1_700_000_500);
    }

    #[test]
    fn a_burst_of_blocks_and_a_halt_reach_the_same_vested_amount() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = vesting(&ctx);

        let mut burst = ScenarioBuilder::new(&ctx);
        burst
            .step("open", |_| client.open(&1_700_000_000, &1_000, &10_000))
            .advance_blocks(100)
            .advance_time(250)
            .step("claim", |_| client.vested());

        // Return the shared env to the starting date so both scenarios cover
        // the same elapsed time rather than the second starting where the first
        // stopped.
        ctx.env.ledger().set_timestamp(1_700_000_000);
        let mut halted = ScenarioBuilder::new(&ctx);
        halted
            .step("open", |_| client.open(&1_700_000_000, &1_000, &10_000))
            .halt(250)
            .advance_blocks(100)
            .step("claim", |_| client.vested());

        burst.assert_all_passed();
        halted.assert_all_passed();
        // Both scenarios end 250 seconds in, so a clock-based contract vests the
        // same amount even though one saw 100 blocks and the other saw a halt.
        assert_eq!(client.vested(), 2_500);
        assert_eq!(
            burst.timeline().last().unwrap().timestamp,
            halted.timeline().last().unwrap().timestamp
        );
    }

    #[test]
    fn a_network_halt_moves_time_without_closing_a_block() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = stream(&ctx);

        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| client.open(&1_700_000_000, &10))
            .halt(3_600)
            .step("claim", |_| client.owed());

        scenario.assert_all_passed();
        assert_eq!(scenario.blocks_elapsed(), 0);
        assert_eq!(scenario.seconds_elapsed(), 3_600);
        assert_eq!(client.owed(), 36_000);
    }

    #[test]
    fn a_rollback_rewinds_the_sequence_and_is_allowed() {
        let ctx = context();
        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .advance_blocks(100)
            .rollback_blocks(40)
            .advance_blocks(5);

        scenario.assert_all_passed();
        assert_eq!(scenario.timeline().len(), 4);
        assert_eq!(scenario.blocks_elapsed(), 65);
        let rollback = &scenario.timeline()[2];
        assert!(rollback.label.contains("rollback 40"));
        assert_eq!(rollback.sequence, 60);
    }

    #[test]
    fn a_panicking_step_is_caught_and_reported_with_the_timeline() {
        let ctx = context();
        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .advance_blocks(10)
            .step("explode", |_| panic!("boom"))
            .step("still runs", |_| ());

        assert_eq!(scenario.failures().len(), 1);
        assert_eq!(scenario.failures()[0].label, "explode");
        assert!(scenario.failures()[0].message.contains("boom"));
        // The step after the panic still ran and is on the timeline.
        assert_eq!(scenario.timeline().last().unwrap().label, "still runs");
        assert_eq!(
            scenario.timeline().last().unwrap().outcome,
            StepOutcome::Passed
        );
    }

    #[test]
    fn assert_all_passed_prints_the_full_timeline() {
        let ctx = context();
        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| ())
            .step("explode", |_| panic!("boom"))
            .step("after", |_| ());

        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scenario.assert_all_passed()
        }));
        let payload = outcome.expect_err("the scenario must fail");
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|text| text.to_string()))
            .unwrap();

        assert!(message.contains("SCENARIO FAILED"));
        assert!(message.contains("explode"));
        assert!(message.contains("#00"));
        assert!(message.contains("after"));
    }

    #[test]
    fn a_failed_consistency_check_is_a_scenario_failure() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = vesting(&ctx);

        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| client.open(&1_700_000_000, &1_000, &10_000))
            .check("nothing vested yet", |_| client.vested() == 0)
            .check("impossible check", |_| client.vested() == 1);

        assert_eq!(scenario.failures().len(), 1);
        assert!(scenario.failures()[0]
            .message
            .contains("consistency check returned false"));
        assert!(scenario.format_timeline().contains("impossible check"));
    }

    #[test]
    fn a_probe_detects_a_contract_seeing_time_move_backwards() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = vesting(&ctx);

        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| client.open(&1_700_000_000, &1_000, &10_000))
            .advance_time(100)
            .probe("before the rollback", |_| client.clock())
            // A reorg rewinds the chain to a moment before the contract opened.
            .set_clock(0, 1_699_999_900)
            .probe("after the rollback", |_| client.clock());

        assert_eq!(scenario.failures().len(), 1);
        let message = &scenario.failures()[0].message;
        assert!(message.contains("clock inversion"));
        assert!(message.contains("underflows"));
    }

    #[test]
    fn a_monotonic_clock_passes_every_probe() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = stream(&ctx);

        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| client.open(&1_700_000_000, &1))
            .probe("at open", |_| client.clock())
            .halt(100)
            .probe("after the halt", |_| client.clock())
            .advance_blocks(10)
            .advance_time(50)
            .probe("after blocks", |_| client.clock());

        scenario.assert_all_passed();
        assert_eq!(scenario.failures().len(), 0);
    }

    #[test]
    fn an_arithmetic_underflow_in_the_contract_fails_the_step() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        let (_id, client) = stream(&ctx);

        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .step("open", |_| client.open(&1_700_001_000, &10))
            .set_clock(0, 1_700_000_500)
            .step("claim after the clock moved back", |_| client.owed());

        assert_eq!(scenario.failures().len(), 1);
        assert_eq!(
            scenario.failures()[0].label,
            "claim after the clock moved back"
        );
        assert!(scenario.format_timeline().contains("FAILED"));
    }

    #[test]
    fn advancing_past_the_end_of_each_range_saturates() {
        let ctx = context();
        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .advance_blocks(u32::MAX)
            .advance_blocks(10)
            .advance_time(u64::MAX)
            .advance_time(10);

        scenario.assert_all_passed();
        assert_eq!(scenario.timeline().last().unwrap().sequence, u32::MAX);
        assert_eq!(scenario.timeline().last().unwrap().timestamp, u64::MAX);
        assert_eq!(scenario.blocks_elapsed(), u32::MAX);
        assert_eq!(scenario.seconds_elapsed(), u64::MAX);
    }

    #[test]
    fn the_timeline_indexes_are_sequential() {
        let ctx = context();
        let mut scenario = ScenarioBuilder::new(&ctx);
        scenario
            .advance_blocks(1)
            .advance_time(1)
            .step("noop", |_| ())
            .halt(1)
            .rollback_blocks(1);

        for (index, entry) in scenario.timeline().iter().enumerate() {
            assert_eq!(entry.index, index);
        }
        assert!(!scenario.is_empty());
        assert_eq!(scenario.len(), 6);
    }

    #[test]
    fn a_fresh_scenario_is_empty_but_has_a_start_entry() {
        let ctx = context();
        let scenario = ScenarioBuilder::new(&ctx);

        assert!(scenario.is_empty());
        assert_eq!(scenario.timeline().len(), 1);
        assert_eq!(scenario.timeline()[0].outcome, StepOutcome::Start);
        assert_eq!(scenario.start_sequence(), 0);
        assert_eq!(scenario.start_timestamp(), 0);
    }

    #[test]
    fn a_clock_that_starts_at_a_date_is_preserved_in_the_summary() {
        let mut ctx = context();
        ctx.set_timestamp(1_700_000_000);
        ctx.advance_ledger(50);

        let scenario = ScenarioBuilder::new(&ctx);
        assert_eq!(scenario.start_timestamp(), 1_700_000_000);
        assert_eq!(scenario.start_sequence(), 50);

        scenario.format_timeline();
    }

    #[test]
    fn a_fresh_env_clock_is_zero_like_the_context() {
        let ctx = context();
        let scenario = ScenarioBuilder::new(&ctx);
        // The builder clones the context's env rather than creating one, so the
        // two see the same ledger.
        assert_eq!(scenario.start_timestamp(), ctx.timestamp());
        assert_eq!(scenario.start_sequence(), ctx.sequence());
    }
}
