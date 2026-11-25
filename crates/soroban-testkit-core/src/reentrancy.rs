//! Reentrancy guard and state access ordering auditing.
//!
//! Smart contracts executing external callbacks, token transfers, or multi-contract
//! routing must ensure that reentrancy guard flags cannot be bypassed through
//! direct or indirect re-entry paths. Static and dynamic verification of state
//! access order guarantees protocol safety under Check-Effects-Interactions (CEI)
//! patterns.
//!
//! [`ReentrancyGuardAuditor`] monitors lock flag states and cross-contract invocations,
//! verifying that:
//! 1. Reentrancy locks are set (acquired) *before* any external invocation is performed.
//! 2. Locks are initialized and not accessed while uninitialized.
//! 3. Locks are not prematurely released before external calls finish.
//! 4. Locks are properly reset upon frame exit.
//!
//! On assertion failure or violation, the auditor renders a visual state access
//! ordering timeline detailing every call step, lock state transition, and the
//! exact sequence that exposed the protocol.
//!
//! [`ReentrancyDetector`] complements the auditor with invoke-time detection of
//! Checks-Effects-Interactions violations and re-entry attempts. It reads host
//! diagnostic `fn_call` / `fn_return` events for call frames and instrumented
//! contract events for storage and call ordering.
//!
//! # Instrumentation topic protocol
//!
//! An instrumented contract emits a contract event whose topics are:
//!
//! 1. `Symbol("tk_reentry")` — protocol marker.
//! 2. `Symbol("storage_read" | "storage_write" | "external_call" | "reentry_attempt")`
//!    — operation.
//! 3. `Symbol(key | function)` — storage key or called function name.
//!
//! The event data carries the other contract as an `Address` for
//! `external_call` and `reentry_attempt`; storage events use `()` data. The
//! emitting contract id comes from the host event itself. The
//! `emit_storage_read`, `emit_storage_write`, `emit_external_call`, and
//! `emit_reentry_attempt` helpers emit exactly this shape.
//!
//! ```rust
//! use soroban_testkit_core::{LockState, ReentrancyGuardAuditor};
//!
//! let mut auditor = ReentrancyGuardAuditor::new();
//! auditor
//!     .enter_frame("CVAULT", "withdraw")
//!     .set_lock("CVAULT", LockState::Locked)
//!     .external_call("CVAULT", "CTOKEN", "transfer")
//!     .enter_frame("CTOKEN", "transfer")
//!     .exit_frame("CTOKEN", "transfer")
//!     .set_lock("CVAULT", LockState::Unlocked)
//!     .exit_frame("CVAULT", "withdraw");
//!
//! let report = auditor.audit();
//! assert!(report.is_safe());
//! ```

use soroban_env_host::events::HostEvent;
use soroban_env_host::xdr::{ContractEventBody, ContractEventType, ScVal};
use soroban_sdk::{
    contract, contractimpl,
    xdr::{ContractId, Hash, ScAddress},
    Address, Env, Symbol, TryFromVal,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// State of a reentrancy guard lock flag in contract storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LockState {
    /// Lock flag has not been initialized in storage.
    Uninitialized,
    /// Guard is set / locked (in-flight execution protected).
    Locked,
    /// Guard is released / cleared (ready for invocations).
    Unlocked,
}

impl LockState {
    /// Human-readable label for the lock state.
    pub fn as_str(&self) -> &'static str {
        match self {
            LockState::Uninitialized => "Uninitialized",
            LockState::Locked => "Locked",
            LockState::Unlocked => "Unlocked",
        }
    }
}

impl fmt::Display for LockState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An individual action or state transition recorded during multi-frame execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionKind {
    /// Entering a contract function call frame.
    EnterFrame { contract: String, function: String },
    /// Setting / updating a reentrancy lock state.
    SetLock {
        contract: String,
        key: String,
        state: LockState,
    },
    /// Reading / checking a reentrancy lock state.
    ReadLock {
        contract: String,
        key: String,
        observed: LockState,
    },
    /// An external call from one contract to another.
    ExternalCall {
        caller: String,
        callee: String,
        function: String,
    },
    /// Exiting a contract function call frame.
    ExitFrame { contract: String, function: String },
}

/// Specific flaw or vulnerability detected by the reentrancy auditor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReentrancyViolationKind {
    /// External call performed while the reentrancy lock is uninitialized.
    UninitializedLock {
        contract: String,
        external_call: String,
        step: usize,
    },
    /// External call performed while the reentrancy lock is unlocked (guard omitted).
    UnlockedExternalCall {
        contract: String,
        external_call: String,
        step: usize,
    },
    /// Reentrancy guard prematurely released before external call completed or frame exited.
    PrematureRelease {
        contract: String,
        external_call: String,
        step: usize,
    },
    /// Reentrancy guard was set to Locked, but never reset upon exiting the frame.
    DanglingLockOnExit {
        contract: String,
        function: String,
        step: usize,
    },
    /// Re-entry attempted into a contract whose frame is still open.
    ReentrancyAttempt {
        contract: String,
        function: String,
        depth: u32,
        step: usize,
    },
}

impl fmt::Display for ReentrancyViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReentrancyViolationKind::UninitializedLock {
                contract,
                external_call,
                step,
            } => {
                write!(
                    f,
                    "[Step {step}] External call from '{contract}' to '{external_call}' while lock is Uninitialized"
                )
            }
            ReentrancyViolationKind::UnlockedExternalCall {
                contract,
                external_call,
                step,
            } => {
                write!(
                    f,
                    "[Step {step}] External call from '{contract}' to '{external_call}' while lock is Unlocked (guard not held)"
                )
            }
            ReentrancyViolationKind::PrematureRelease {
                contract,
                external_call,
                step,
            } => {
                write!(
                    f,
                    "[Step {step}] Lock on '{contract}' was prematurely released before external call '{external_call}' resolved"
                )
            }
            ReentrancyViolationKind::DanglingLockOnExit {
                contract,
                function,
                step,
            } => {
                write!(
                    f,
                    "[Step {step}] Frame '{contract}::{function}' exited while reentrancy lock is still Locked"
                )
            }
            ReentrancyViolationKind::ReentrancyAttempt {
                contract,
                function,
                depth,
                step,
            } => {
                write!(
                    f,
                    "[Step {step}] Reentrant invocation of '{contract}::{function}' (depth {depth}) while active frame is still open"
                )
            }
        }
    }
}

/// A diagnosed reentrancy violation with explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReentrancyViolation {
    pub kind: ReentrancyViolationKind,
    pub description: String,
}

impl fmt::Display for ReentrancyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.description)
    }
}

/// A recorded step in the state access ordering timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineStep {
    pub step: usize,
    pub depth: u32,
    pub contract: String,
    pub action_desc: String,
    pub lock_state: Option<LockState>,
    pub violation: Option<String>,
}

/// Report containing the results and timeline of a reentrancy guard audit.
#[derive(Debug, Clone, Default)]
pub struct ReentrancyReport {
    violations: Vec<ReentrancyViolation>,
    timeline: Vec<TimelineStep>,
}

impl ReentrancyReport {
    /// True if no reentrancy vulnerabilities or access order flaws were detected.
    pub fn is_safe(&self) -> bool {
        self.violations.is_empty()
    }

    /// True if any violations were identified.
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }

    /// All identified reentrancy violations.
    pub fn violations(&self) -> &[ReentrancyViolation] {
        &self.violations
    }

    /// All timeline steps recorded.
    pub fn timeline_steps(&self) -> &[TimelineStep] {
        &self.timeline
    }

    /// Formats a visual ASCII state access ordering timeline detailing the execution path.
    pub fn render_timeline(&self) -> String {
        let mut out = String::new();
        out.push_str("====================================================================================================\n");
        out.push_str("                        STATE ACCESS ORDERING & REENTRANCY TIMELINE\n");
        out.push_str("====================================================================================================\n");
        out.push_str(&format!(
            "{:<4} | {:<5} | {:<10} | {:<37} | {:<13} | {}\n",
            "Step", "Depth", "Contract", "Action", "Lock State", "Status / Notes"
        ));
        out.push_str("-----+-------+------------+---------------------------------------+---------------+-----------------\n");

        for step in &self.timeline {
            let lock_str = step
                .lock_state
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string());
            let status_str = match &step.violation {
                Some(v) => format!("[VIOLATION] {v}"),
                None => "OK".to_string(),
            };

            let contract_trunc = if step.contract.len() > 10 {
                format!("{}...", &step.contract[..7])
            } else {
                step.contract.clone()
            };

            let action_trunc = if step.action_desc.len() > 37 {
                format!("{}...", &step.action_desc[..34])
            } else {
                step.action_desc.clone()
            };

            out.push_str(&format!(
                "{:<4} | {:<5} | {:<10} | {:<37} | {:<13} | {}\n",
                step.step, step.depth, contract_trunc, action_trunc, lock_str, status_str
            ));
        }

        out.push_str("----------------------------------------------------------------------------------------------------\n");
        if self.violations.is_empty() {
            out.push_str("Result: PASS — No reentrancy flaws detected. Locks set before external calls and reset on exit.\n");
        } else {
            out.push_str(&format!(
                "Result: FAILED — {} reentrancy violation(s) detected:\n",
                self.violations.len()
            ));
            for (i, v) in self.violations.iter().enumerate() {
                out.push_str(&format!("  {}. {}\n", i + 1, v.kind));
            }
        }
        out.push_str("====================================================================================================\n");
        out
    }
}

/// An open call frame tracked on the auditor's call stack.
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct ActiveFrame {
    contract: String,
    function: String,
    depth: u32,
    lock_state_at_entry: LockState,
    lock_set_in_frame: bool,
    external_calls_made: Vec<String>,
}

/// Auditor for statically and dynamically verifying reentrancy guard locks and state access order.
#[derive(Debug, Clone)]
pub struct ReentrancyGuardAuditor {
    /// Default key used to identify reentrancy lock storage flags.
    default_lock_key: String,
    /// Explicit monitored contracts (if empty, all contracts in trace are audited).
    monitored_contracts: BTreeSet<String>,
    /// Active call stack frames.
    call_stack: Vec<ActiveFrame>,
    /// Persistent lock state per contract.
    contract_locks: BTreeMap<String, LockState>,
    /// Recorded actions in sequential execution order.
    actions: Vec<ActionKind>,
    /// Step counter.
    step_count: usize,
    /// Diagnostic events cursor for dynamic observation.
    consumed_events: usize,
}

impl Default for ReentrancyGuardAuditor {
    fn default() -> Self {
        Self::new()
    }
}

impl ReentrancyGuardAuditor {
    /// Create a new reentrancy guard auditor.
    pub fn new() -> Self {
        Self {
            default_lock_key: "locked".to_string(),
            monitored_contracts: BTreeSet::new(),
            call_stack: Vec::new(),
            contract_locks: BTreeMap::new(),
            actions: Vec::new(),
            step_count: 0,
            consumed_events: 0,
        }
    }

    /// Configure a custom storage key name for reentrancy guard flags (default: `"locked"`).
    pub fn with_lock_key(mut self, key: impl Into<String>) -> Self {
        self.default_lock_key = key.into();
        self
    }

    /// Monitor a specific contract ID. If any contracts are explicitly added,
    /// the auditor only evaluates safety for those contracts.
    pub fn with_monitored_contract(mut self, contract: impl Into<String>) -> Self {
        self.monitored_contracts.insert(contract.into());
        self
    }

    /// Record entering a contract function frame.
    pub fn enter_frame(
        &mut self,
        contract: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let contract = contract.into();
        let function = function.into();
        self.step_count += 1;

        let depth = self.call_stack.len() as u32 + 1;
        let lock_state = self
            .contract_locks
            .get(&contract)
            .copied()
            .unwrap_or(LockState::Uninitialized);

        self.call_stack.push(ActiveFrame {
            contract: contract.clone(),
            function: function.clone(),
            depth,
            lock_state_at_entry: lock_state,
            lock_set_in_frame: false,
            external_calls_made: Vec::new(),
        });

        self.actions
            .push(ActionKind::EnterFrame { contract, function });
        self
    }

    /// Record setting or updating a contract's reentrancy lock state.
    pub fn set_lock(&mut self, contract: impl Into<String>, state: LockState) -> &mut Self {
        let default_key = self.default_lock_key.clone();
        self.set_lock_with_key(contract, default_key, state)
    }

    /// Record setting a reentrancy lock under a specific storage key.
    pub fn set_lock_with_key(
        &mut self,
        contract: impl Into<String>,
        key: impl Into<String>,
        state: LockState,
    ) -> &mut Self {
        let contract = contract.into();
        let key = key.into();
        self.step_count += 1;

        self.contract_locks.insert(contract.clone(), state);

        if let Some(frame) = self.call_stack.iter_mut().rfind(|f| f.contract == contract) {
            if state == LockState::Locked {
                frame.lock_set_in_frame = true;
            }
        }

        self.actions.push(ActionKind::SetLock {
            contract,
            key,
            state,
        });
        self
    }

    /// Record reading / checking a reentrancy lock state.
    pub fn read_lock(&mut self, contract: impl Into<String>) -> &mut Self {
        let default_key = self.default_lock_key.clone();
        let contract = contract.into();
        self.step_count += 1;

        let observed = self
            .contract_locks
            .get(&contract)
            .copied()
            .unwrap_or(LockState::Uninitialized);

        self.actions.push(ActionKind::ReadLock {
            contract,
            key: default_key,
            observed,
        });
        self
    }

    /// Record an external invocation from one contract to another.
    pub fn external_call(
        &mut self,
        caller: impl Into<String>,
        callee: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let caller = caller.into();
        let callee = callee.into();
        let function = function.into();
        self.step_count += 1;

        if let Some(frame) = self.call_stack.iter_mut().rfind(|f| f.contract == caller) {
            frame
                .external_calls_made
                .push(format!("{callee}::{function}"));
        }

        self.actions.push(ActionKind::ExternalCall {
            caller,
            callee,
            function,
        });
        self
    }

    /// Record exiting a contract function frame.
    pub fn exit_frame(
        &mut self,
        contract: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let contract = contract.into();
        let function = function.into();
        self.step_count += 1;

        self.call_stack.pop();
        self.actions
            .push(ActionKind::ExitFrame { contract, function });
        self
    }

    /// Observe the diagnostic events already recorded in `env` since the last call.
    pub fn observe(&mut self, env: &Env) -> &mut Self {
        let events = match env.host().get_diagnostic_events() {
            Ok(events) => events.0,
            Err(_) => return self,
        };

        for host_event in events.iter().skip(self.consumed_events) {
            let event = &host_event.event;
            if event.type_ != ContractEventType::Diagnostic {
                continue;
            }
            let ContractEventBody::V0(body) = &event.body;
            let topics = &body.topics;

            let Some(ScVal::Symbol(kind)) = topics.first() else {
                continue;
            };

            // Diagnostic event: fn_call
            if kind.0.as_slice() == b"fn_call" {
                if let (Some(ScVal::Bytes(callee_bytes)), Some(ScVal::Symbol(func_sym))) =
                    (topics.get(1), topics.get(2))
                {
                    if let Ok(callee_hash) = <[u8; 32]>::try_from(callee_bytes.0.as_slice()) {
                        let callee = address_label(env, callee_hash);
                        let func_str = String::from_utf8_lossy(&func_sym.0).to_string();

                        if let Some(caller_id) = &event.contract_id {
                            let caller = address_label(env, (caller_id.0).0);
                            self.external_call(&caller, &callee, &func_str);
                        }

                        self.enter_frame(&callee, &func_str);
                    }
                }
            } else if kind.0.as_slice() == b"fn_return" {
                // Diagnostic event: fn_return
                if let (Some(caller_id), Some(ScVal::Symbol(func_sym))) =
                    (&event.contract_id, topics.get(1))
                {
                    let contract = address_label(env, (caller_id.0).0);
                    let func_str = String::from_utf8_lossy(&func_sym.0).to_string();
                    self.exit_frame(&contract, &func_str);
                }
            }
        }

        self.consumed_events = events.len();
        self
    }

    /// Create an auditor and observe all diagnostic events in `env`.
    pub fn from_env(env: &Env) -> Self {
        let mut auditor = Self::new();
        auditor.observe(env);
        auditor
    }

    /// Runs reentrancy safety and state access ordering analysis on the recorded trace.
    pub fn audit(&self) -> ReentrancyReport {
        let mut violations = Vec::new();
        let mut timeline = Vec::new();

        // State machine simulation over actions
        let mut simulated_locks: BTreeMap<String, LockState> = BTreeMap::new();
        let mut open_frames: Vec<(String, String, u32)> = Vec::new();
        let mut external_calls_in_frame: Vec<Vec<String>> = Vec::new();

        for (idx, action) in self.actions.iter().enumerate() {
            let step = idx + 1;
            let current_depth = open_frames.len() as u32;

            match action {
                ActionKind::EnterFrame { contract, function } => {
                    let depth = current_depth + 1;
                    let lock_state = simulated_locks
                        .get(contract)
                        .copied()
                        .unwrap_or(LockState::Uninitialized);

                    let mut violation_note = None;

                    // Check for direct or indirect re-entry into an already-open frame
                    if self.should_audit(contract) {
                        if let Some((_, _, open_depth)) =
                            open_frames.iter().find(|(c, _, _)| c == contract)
                        {
                            let v_kind = ReentrancyViolationKind::ReentrancyAttempt {
                                contract: contract.clone(),
                                function: function.clone(),
                                depth,
                                step,
                            };
                            let note = format!(
                                "Reentrant invocation while frame at depth {open_depth} is open"
                            );
                            violations.push(ReentrancyViolation {
                                kind: v_kind,
                                description: note.clone(),
                            });
                            violation_note = Some(note);
                        }
                    }

                    open_frames.push((contract.clone(), function.clone(), depth));
                    external_calls_in_frame.push(Vec::new());

                    timeline.push(TimelineStep {
                        step,
                        depth,
                        contract: contract.clone(),
                        action_desc: format!("Enter: {function}"),
                        lock_state: Some(lock_state),
                        violation: violation_note,
                    });
                }
                ActionKind::SetLock {
                    contract,
                    key: _,
                    state,
                } => {
                    let prev = simulated_locks
                        .insert(contract.clone(), *state)
                        .unwrap_or(LockState::Uninitialized);

                    let mut violation_note = None;

                    // Premature release check:
                    // If the lock was set to Unlocked while an external call is still in flight
                    // (i.e. this contract has an open frame and there are nested frames on top of it).
                    if self.should_audit(contract)
                        && (*state == LockState::Unlocked || *state == LockState::Uninitialized)
                        && prev == LockState::Locked
                    {
                        let is_external_call_in_flight = open_frames
                            .iter()
                            .position(|(c, _, _)| c == contract)
                            .map(|pos| pos + 1 < open_frames.len())
                            .unwrap_or(false);

                        if is_external_call_in_flight {
                            let ext_call_desc = open_frames
                                .last()
                                .map(|(c, f, _)| format!("{c}::{f}"))
                                .unwrap_or_else(|| "external call".to_string());
                            let v_kind = ReentrancyViolationKind::PrematureRelease {
                                contract: contract.clone(),
                                external_call: ext_call_desc.clone(),
                                step,
                            };
                            let note = format!(
                                "Lock on '{contract}' was prematurely released while external call '{ext_call_desc}' is still in flight"
                            );
                            violations.push(ReentrancyViolation {
                                kind: v_kind,
                                description: note.clone(),
                            });
                            violation_note = Some(note);
                        }
                    }

                    timeline.push(TimelineStep {
                        step,
                        depth: current_depth,
                        contract: contract.clone(),
                        action_desc: format!("SetLock: {state}"),
                        lock_state: Some(*state),
                        violation: violation_note,
                    });
                }
                ActionKind::ReadLock {
                    contract,
                    key: _,
                    observed,
                } => {
                    timeline.push(TimelineStep {
                        step,
                        depth: current_depth,
                        contract: contract.clone(),
                        action_desc: format!("ReadLock: {observed}"),
                        lock_state: Some(*observed),
                        violation: None,
                    });
                }
                ActionKind::ExternalCall {
                    caller,
                    callee,
                    function,
                } => {
                    let mut violation_note = None;

                    if let Some(ext_calls) = external_calls_in_frame.last_mut() {
                        ext_calls.push(format!("{callee}::{function}"));
                    }

                    // Check CEI: Reentrancy lock MUST be Locked before an external call!
                    if self.should_audit(caller) {
                        let lock = simulated_locks
                            .get(caller)
                            .copied()
                            .unwrap_or(LockState::Uninitialized);

                        match lock {
                            LockState::Uninitialized => {
                                let v_kind = ReentrancyViolationKind::UninitializedLock {
                                    contract: caller.clone(),
                                    external_call: format!("{callee}::{function}"),
                                    step,
                                };
                                let note = format!("External call made while lock flag is Uninitialized on '{caller}'");
                                violations.push(ReentrancyViolation {
                                    kind: v_kind,
                                    description: note.clone(),
                                });
                                violation_note = Some(note);
                            }
                            LockState::Unlocked => {
                                let v_kind = ReentrancyViolationKind::UnlockedExternalCall {
                                    contract: caller.clone(),
                                    external_call: format!("{callee}::{function}"),
                                    step,
                                };
                                let note = format!("External call made while lock flag is Unlocked (guard not held) on '{caller}'");
                                violations.push(ReentrancyViolation {
                                    kind: v_kind,
                                    description: note.clone(),
                                });
                                violation_note = Some(note);
                            }
                            LockState::Locked => {
                                // Correct behavior: lock is active before external call
                            }
                        }
                    }

                    timeline.push(TimelineStep {
                        step,
                        depth: current_depth,
                        contract: caller.clone(),
                        action_desc: format!("ExternalCall -> {callee}::{function}"),
                        lock_state: simulated_locks.get(caller).copied(),
                        violation: violation_note,
                    });
                }
                ActionKind::ExitFrame { contract, function } => {
                    let depth = current_depth;
                    open_frames.pop();
                    external_calls_in_frame.pop();

                    let current_lock = simulated_locks
                        .get(contract)
                        .copied()
                        .unwrap_or(LockState::Uninitialized);

                    let mut violation_note = None;

                    // Check for dangling lock: If frame exits but lock is still Locked!
                    if self.should_audit(contract) && current_lock == LockState::Locked {
                        let v_kind = ReentrancyViolationKind::DanglingLockOnExit {
                            contract: contract.clone(),
                            function: function.clone(),
                            step,
                        };
                        let note = format!(
                            "Frame '{contract}::{function}' exited with lock still in Locked state"
                        );
                        violations.push(ReentrancyViolation {
                            kind: v_kind,
                            description: note.clone(),
                        });
                        violation_note = Some(note);
                    }

                    timeline.push(TimelineStep {
                        step,
                        depth,
                        contract: contract.clone(),
                        action_desc: format!("Exit: {function}"),
                        lock_state: Some(current_lock),
                        violation: violation_note,
                    });
                }
            }
        }

        ReentrancyReport {
            violations,
            timeline,
        }
    }

    /// Asserts that no reentrancy vulnerabilities exist; panics with a visual timeline if violated.
    pub fn assert_reentrancy_safe(&self) {
        let report = self.audit();
        if report.has_violations() {
            panic!(
                "Reentrancy guard safety assertion failed!\n\n{}",
                report.render_timeline()
            );
        }
    }

    /// Render visual timeline directly from the auditor.
    pub fn render_timeline(&self) -> String {
        self.audit().render_timeline()
    }

    fn should_audit(&self, contract: &str) -> bool {
        if self.monitored_contracts.is_empty() {
            true
        } else {
            self.monitored_contracts.contains(contract)
        }
    }
}

/// Marker topic for the reentrancy instrumentation protocol.
pub const REENTRANCY_EVENT_TOPIC: &str = "tk_reentry";
/// Operation topic for a storage read.
pub const REENTRANCY_STORAGE_READ: &str = "storage_read";
/// Operation topic for a storage write.
pub const REENTRANCY_STORAGE_WRITE: &str = "storage_write";
/// Operation topic for a cross-contract call.
pub const REENTRANCY_EXTERNAL_CALL: &str = "external_call";
/// Operation topic for a simulated re-entry attempt.
pub const REENTRANCY_REENTRY_ATTEMPT: &str = "reentry_attempt";

/// Emit a storage-read instrumentation event from a contract.
#[allow(deprecated)]
pub fn emit_storage_read(env: &Env, key: Symbol) {
    env.events().publish(
        (
            Symbol::new(env, REENTRANCY_EVENT_TOPIC),
            Symbol::new(env, REENTRANCY_STORAGE_READ),
            key,
        ),
        (),
    );
}

/// Emit a storage-write instrumentation event from a contract.
#[allow(deprecated)]
pub fn emit_storage_write(env: &Env, key: Symbol) {
    env.events().publish(
        (
            Symbol::new(env, REENTRANCY_EVENT_TOPIC),
            Symbol::new(env, REENTRANCY_STORAGE_WRITE),
            key,
        ),
        (),
    );
}

/// Emit a cross-contract-call instrumentation event from a contract.
#[allow(deprecated)]
pub fn emit_external_call(env: &Env, callee: &Address, function: Symbol) {
    env.events().publish(
        (
            Symbol::new(env, REENTRANCY_EVENT_TOPIC),
            Symbol::new(env, REENTRANCY_EXTERNAL_CALL),
            function,
        ),
        callee.clone(),
    );
}

/// Emit a simulated re-entry attempt from a contract.
#[allow(deprecated)]
pub fn emit_reentry_attempt(env: &Env, target: &Address, function: Symbol) {
    env.events().publish(
        (
            Symbol::new(env, REENTRANCY_EVENT_TOPIC),
            Symbol::new(env, REENTRANCY_REENTRY_ATTEMPT),
            function,
        ),
        target.clone(),
    );
}

/// Attacker contract that publishes a re-entry attempt against a target contract.
///
/// The host refuses an actual recursive invocation before a nested frame opens,
/// so the attacker emits a protocol event instead. [`ReentrancyDetector`] reads
/// that event and reports the attempt when the target is already on the active
/// call stack.
#[contract]
pub struct MockReentrantAttacker;

#[contractimpl]
impl MockReentrantAttacker {
    /// Publish an attempt to re-enter `target::function`.
    pub fn attempt_reentry(env: Env, target: Address, function: Symbol) {
        emit_reentry_attempt(&env, &target, function);
    }

    /// Alias of [`MockReentrantAttacker::attempt_reentry`].
    pub fn attack(env: Env, target: Address, function: Symbol) {
        emit_reentry_attempt(&env, &target, function);
    }

    /// Callback-shaped alias of [`MockReentrantAttacker::attempt_reentry`].
    pub fn callback(env: Env, target: Address, function: Symbol) {
        emit_reentry_attempt(&env, &target, function);
    }
}

/// Kind of flaw found by [`ReentrancyDetector`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectorViolationKind {
    /// A frame called out before writing its state.
    CeiViolation {
        contract: String,
        function: String,
        external_call: String,
        depth: u32,
        step: usize,
    },
    /// A contract reappeared on the active call stack.
    ReentrancyAttempt {
        contract: String,
        function: String,
        depth: u32,
        step: usize,
    },
}

impl DetectorViolationKind {
    /// True for a Checks-Effects-Interactions ordering violation.
    pub fn is_cei_violation(&self) -> bool {
        matches!(self, Self::CeiViolation { .. })
    }

    /// True for a re-entry attempt.
    pub fn is_reentrancy_attempt(&self) -> bool {
        matches!(self, Self::ReentrancyAttempt { .. })
    }
}

impl fmt::Display for DetectorViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CeiViolation {
                contract,
                function,
                external_call,
                depth,
                step,
            } => write!(
                f,
                "[Step {step}] CEI violation in '{contract}::{function}' at depth {depth}: external call '{external_call}' preceded any storage write"
            ),
            Self::ReentrancyAttempt {
                contract,
                function,
                depth,
                step,
            } => write!(
                f,
                "[Step {step}] Re-entry attempt into '{contract}::{function}' at depth {depth} while the contract is already active"
            ),
        }
    }
}

/// A violation with its monitor label and human-readable explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorViolation {
    pub kind: DetectorViolationKind,
    pub label: Option<String>,
    pub description: String,
}

impl fmt::Display for DetectorViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.label {
            Some(label) => write!(f, "{} [label={label}]", self.kind),
            None => write!(f, "{}", self.kind),
        }
    }
}

#[derive(Debug, Clone)]
struct DetectorFrame {
    contract: String,
    function: String,
    depth: u32,
    reads: Vec<String>,
    writes: Vec<String>,
    calls: Vec<String>,
}

impl DetectorFrame {
    fn new(contract: String, function: String, depth: u32) -> Self {
        Self {
            contract,
            function,
            depth,
            reads: Vec::new(),
            writes: Vec::new(),
            calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct DetectorTraceEntry {
    step: usize,
    depth: u32,
    label: Option<String>,
    contract: String,
    description: String,
}

/// Detects CEI violations and re-entry attempts during an invocation.
///
/// The detector owns a clone of the test `Env`, so [`ReentrancyDetector::monitor`]
/// can observe the events produced by a closure without being passed the
/// environment again. Frames come from host diagnostic events; storage and call
/// ordering come from the instrumentation protocol documented at the module
/// level. Manual `record_*` and frame methods are available for harnesses that
/// cannot publish contract events.
#[derive(Clone)]
pub struct ReentrancyDetector {
    env: Env,
    consumed_events: usize,
    step: usize,
    label: Option<String>,
    frames: Vec<DetectorFrame>,
    unframed: BTreeMap<String, DetectorFrame>,
    trace: Vec<DetectorTraceEntry>,
    violations: Vec<DetectorViolation>,
}

impl ReentrancyDetector {
    /// Create a detector that reads host events from `env`.
    pub fn new(env: &Env) -> Self {
        Self {
            env: env.clone(),
            consumed_events: 0,
            step: 0,
            label: None,
            frames: Vec::new(),
            unframed: BTreeMap::new(),
            trace: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// Run `closure` under `label` and observe only the events it produces.
    pub fn monitor<T, F>(&mut self, label: impl Into<String>, closure: F) -> T
    where
        F: FnOnce() -> T,
    {
        self.consumed_events = 0;
        self.label = Some(label.into());
        let result = closure();
        self.observe();
        self.label = None;
        result
    }

    /// Observe diagnostic and instrumentation events produced since the last observation.
    pub fn observe(&mut self) -> &mut Self {
        let events = match self.env.host().get_events() {
            Ok(events) => events.0,
            Err(_) => return self,
        };

        for host_event in events.iter().skip(self.consumed_events) {
            self.observe_event(host_event);
        }
        self.consumed_events = events.len();
        self
    }

    /// Switch the environment to observe and ingest its new events.
    pub fn observe_env(&mut self, env: &Env) -> &mut Self {
        self.env = env.clone();
        self.observe()
    }

    /// Record a storage read by `contract`.
    pub fn record_storage_read(
        &mut self,
        contract: impl Into<String>,
        key: impl Into<String>,
    ) -> &mut Self {
        let contract = contract.into();
        let key = key.into();
        let depth = {
            let frame = self.frame_for(&contract);
            frame.reads.push(key.clone());
            frame.depth
        };
        self.record_trace(&contract, depth, format!("storage read: {key}"));
        self
    }

    /// Record a storage write by `contract`.
    pub fn record_storage_write(
        &mut self,
        contract: impl Into<String>,
        key: impl Into<String>,
    ) -> &mut Self {
        let contract = contract.into();
        let key = key.into();
        let depth = {
            let frame = self.frame_for(&contract);
            frame.writes.push(key.clone());
            frame.depth
        };
        self.record_trace(&contract, depth, format!("storage write: {key}"));
        self
    }

    /// Record a call from `caller` to `callee::function`.
    pub fn record_external_call(
        &mut self,
        caller: impl Into<String>,
        callee: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let caller = caller.into();
        let callee = callee.into();
        let function = function.into();
        let call = format!("{callee}::{function}");
        let (depth, current_function, reads, writes, has_write) = {
            let frame = self.frame_for(&caller);
            frame.calls.push(call.clone());
            (
                frame.depth,
                frame.function.clone(),
                frame.reads.len(),
                frame.writes.len(),
                !frame.writes.is_empty(),
            )
        };
        let step = self.record_trace(
            &caller,
            depth,
            format!("external call -> {call} ({reads} read(s), {writes} write(s))"),
        );

        if !has_write {
            let description = format!(
                "'{caller}::{current_function}' called '{call}' before writing any state in this frame"
            );
            self.push_violation(
                DetectorViolationKind::CeiViolation {
                    contract: caller.clone(),
                    function: current_function,
                    external_call: call,
                    depth,
                    step,
                },
                description,
            );
        }
        self
    }

    /// Record a simulated attempt to re-enter `target::function`.
    pub fn record_reentry_attempt(
        &mut self,
        caller: impl Into<String>,
        target: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let caller = caller.into();
        let target = target.into();
        let function = function.into();
        let active_depth = self
            .frames
            .iter()
            .position(|frame| frame.contract == target)
            .map(|position| position as u32 + 1);
        let depth = active_depth.unwrap_or_else(|| self.frames.len() as u32 + 1);
        let step = self.record_trace(
            &caller,
            depth,
            format!("reentry attempt -> {target}::{function}"),
        );

        if active_depth.is_some() {
            let description = format!(
                "'{caller}' attempted to re-enter active contract '{target}::{function}' at depth {depth}"
            );
            self.push_violation(
                DetectorViolationKind::ReentrancyAttempt {
                    contract: target,
                    function,
                    depth,
                    step,
                },
                description,
            );
        }
        self
    }

    /// Record entering `contract::function`.
    pub fn enter_frame(
        &mut self,
        contract: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let contract = contract.into();
        let function = function.into();
        let reentered = self.frames.iter().any(|frame| frame.contract == contract);
        let depth = self.frames.len() as u32 + 1;
        self.frames.push(DetectorFrame::new(
            contract.clone(),
            function.clone(),
            depth,
        ));
        let step = self.record_trace(&contract, depth, format!("enter {contract}::{function}"));

        if reentered {
            let description = format!(
                "'{contract}' re-entered at depth {depth} while an earlier frame was still active"
            );
            self.push_violation(
                DetectorViolationKind::ReentrancyAttempt {
                    contract,
                    function,
                    depth,
                    step,
                },
                description,
            );
        }
        self
    }

    /// Record exiting the matching contract frame.
    pub fn exit_frame(
        &mut self,
        contract: impl Into<String>,
        function: impl Into<String>,
    ) -> &mut Self {
        let contract = contract.into();
        let function = function.into();
        let position = self
            .frames
            .iter()
            .rposition(|frame| frame.contract == contract && frame.function == function)
            .or_else(|| {
                self.frames
                    .iter()
                    .rposition(|frame| frame.contract == contract)
            });

        let (depth, calls) = match position {
            Some(position) => {
                let frame = self.frames.remove(position);
                (frame.depth, frame.calls.len())
            }
            None => (self.frames.len() as u32, 0),
        };
        self.record_trace(
            &contract,
            depth,
            format!("exit {contract}::{function} ({calls} call(s))"),
        );
        self
    }

    /// True if any frame called out before writing state.
    pub fn has_cei_violations(&self) -> bool {
        self.violations
            .iter()
            .any(|violation| violation.kind.is_cei_violation())
    }

    /// True if a contract reappeared on the active stack or attempted to.
    pub fn has_reentrancy_attempts(&self) -> bool {
        self.violations
            .iter()
            .any(|violation| violation.kind.is_reentrancy_attempt())
    }

    /// True if any violation was recorded.
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }

    /// All recorded violations.
    pub fn violations(&self) -> &[DetectorViolation] {
        &self.violations
    }

    /// Render the observed trace and every violation as a report.
    pub fn report(&self) -> String {
        let mut out = String::new();
        out.push_str("=== REENTRANCY DETECTION REPORT ===\n");
        out.push_str("Trace:\n");
        for entry in &self.trace {
            out.push_str(&format!(
                "  [Step {}] depth={} label={} {} :: {}\n",
                entry.step,
                entry.depth,
                entry.label.as_deref().unwrap_or("-"),
                entry.contract,
                entry.description
            ));
        }
        out.push_str("Violations:\n");
        if self.violations.is_empty() {
            out.push_str("  none\n");
        } else {
            for (index, violation) in self.violations.iter().enumerate() {
                out.push_str(&format!(
                    "  {}. {}\n     {}\n",
                    index + 1,
                    violation,
                    violation.description
                ));
            }
        }
        out.push_str(&format!(
            "Result: {}\n",
            if self.has_violations() {
                "UNSAFE"
            } else {
                "SAFE"
            }
        ));
        out
    }

    /// Panic with the report when any violation was recorded.
    #[track_caller]
    pub fn assert_safe(&self) {
        assert!(
            !self.has_violations(),
            "Reentrancy safety assertion failed\n{}",
            self.report()
        );
    }

    fn observe_event(&mut self, host_event: &HostEvent) {
        let event = &host_event.event;
        let ContractEventBody::V0(body) = &event.body;
        let Some(ScVal::Symbol(topic)) = body.topics.first() else {
            return;
        };

        match event.type_ {
            ContractEventType::Diagnostic => match topic.0.as_slice() {
                b"fn_call" => {
                    let (Some(callee_hash), Some(function)) =
                        (bytes32_at(&body.topics, 1), symbol_at(&body.topics, 2))
                    else {
                        return;
                    };
                    let callee = address_label(&self.env, callee_hash);
                    if let Some(caller_id) = &event.contract_id {
                        let caller = address_label(&self.env, (caller_id.0).0);
                        if caller != callee {
                            self.record_external_call(caller, callee.clone(), function.clone());
                        }
                    }
                    self.enter_frame(callee, function);
                }
                b"fn_return" => {
                    if let (Some(contract_id), Some(function)) =
                        (&event.contract_id, symbol_at(&body.topics, 1))
                    {
                        let contract = address_label(&self.env, (contract_id.0).0);
                        self.exit_frame(contract, function);
                    }
                }
                _ => {}
            },
            ContractEventType::System => {}
            ContractEventType::Contract => {
                if topic.0.as_slice() != REENTRANCY_EVENT_TOPIC.as_bytes() {
                    return;
                }
                let (Some(operation), Some(field), Some(contract_id)) = (
                    symbol_at(&body.topics, 1),
                    symbol_at(&body.topics, 2),
                    event.contract_id.as_ref(),
                ) else {
                    return;
                };
                let contract = address_label(&self.env, (contract_id.0).0);
                let target = event_value_label(&self.env, &body.data);

                match operation.as_bytes() {
                    b"storage_read" | b"read" => {
                        self.record_storage_read(contract, field);
                    }
                    b"storage_write" | b"write" => {
                        self.record_storage_write(contract, field);
                    }
                    b"external_call" | b"call" => {
                        let callee = if target.is_empty() {
                            symbol_at(&body.topics, 3).unwrap_or_else(|| "<unknown>".to_string())
                        } else {
                            target
                        };
                        self.record_external_call(contract, callee, field);
                    }
                    b"reentry_attempt" | b"reenter" => {
                        let attempted = if target.is_empty() {
                            symbol_at(&body.topics, 3).unwrap_or_else(|| "<unknown>".to_string())
                        } else {
                            target
                        };
                        self.record_reentry_attempt(contract, attempted, field);
                    }
                    _ => {}
                }
            }
        }
    }

    fn frame_for(&mut self, contract: &str) -> &mut DetectorFrame {
        if let Some(position) = self
            .frames
            .iter()
            .rposition(|frame| frame.contract == contract)
        {
            return &mut self.frames[position];
        }
        self.unframed
            .entry(contract.to_string())
            .or_insert_with(|| {
                DetectorFrame::new(contract.to_string(), "<unframed>".to_string(), 0)
            })
    }

    fn record_trace(
        &mut self,
        contract: impl Into<String>,
        depth: u32,
        description: impl Into<String>,
    ) -> usize {
        self.step += 1;
        let step = self.step;
        self.trace.push(DetectorTraceEntry {
            step,
            depth,
            label: self.label.clone(),
            contract: contract.into(),
            description: description.into(),
        });
        step
    }

    fn push_violation(&mut self, kind: DetectorViolationKind, description: String) {
        self.violations.push(DetectorViolation {
            label: self.label.clone(),
            kind,
            description,
        });
    }
}

fn bytes32_at(topics: &[ScVal], index: usize) -> Option<[u8; 32]> {
    let ScVal::Bytes(bytes) = topics.get(index)? else {
        return None;
    };
    bytes.0.as_slice().try_into().ok()
}

fn symbol_at(topics: &[ScVal], index: usize) -> Option<String> {
    match topics.get(index)? {
        ScVal::Symbol(symbol) => Some(String::from_utf8_lossy(symbol.0.as_slice()).into_owned()),
        ScVal::String(value) => Some(String::from_utf8_lossy(value.0.as_slice()).into_owned()),
        _ => None,
    }
}

fn event_value_label(env: &Env, value: &ScVal) -> String {
    match value {
        ScVal::Address(ScAddress::Contract(id)) => address_label(env, (id.0).0),
        ScVal::Symbol(symbol) => String::from_utf8_lossy(symbol.0.as_slice()).into_owned(),
        ScVal::String(value) => String::from_utf8_lossy(value.0.as_slice()).into_owned(),
        ScVal::Bytes(bytes) => {
            if bytes.0.len() == 32 {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(bytes.0.as_slice());
                address_label(env, hash)
            } else {
                format!("bytes[{}]", bytes.0.len())
            }
        }
        _ => String::new(),
    }
}

fn address_label(env: &Env, hash: [u8; 32]) -> String {
    let address = ScAddress::Contract(ContractId(Hash(hash)));
    Address::try_from_val(env, &address)
        .map(|addr| format!("{}", addr.to_string()))
        .unwrap_or_else(|_| {
            hash.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_reentrancy_guard_passes() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "withdraw")
            .set_lock("CVAULT", LockState::Locked)
            .external_call("CVAULT", "CTOKEN", "transfer")
            .enter_frame("CTOKEN", "transfer")
            .exit_frame("CTOKEN", "transfer")
            .set_lock("CVAULT", LockState::Unlocked)
            .exit_frame("CVAULT", "withdraw");

        let report = auditor.audit();
        assert!(report.is_safe());
        assert!(!report.has_violations());
        assert_eq!(report.violations().len(), 0);

        let timeline = report.render_timeline();
        assert!(timeline.contains("Result: PASS"));
        assert!(timeline.contains("CVAULT"));
        assert!(timeline.contains("CTOKEN"));
    }

    #[test]
    fn test_uninitialized_lock_before_external_call_flagged() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "withdraw")
            // Flaw: Call external contract BEFORE setting lock
            .external_call("CVAULT", "CTOKEN", "transfer")
            .enter_frame("CTOKEN", "transfer")
            .exit_frame("CTOKEN", "transfer")
            .set_lock("CVAULT", LockState::Locked)
            .set_lock("CVAULT", LockState::Unlocked)
            .exit_frame("CVAULT", "withdraw");

        let report = auditor.audit();
        assert!(!report.is_safe());
        assert_eq!(report.violations().len(), 1);

        let v = &report.violations()[0];
        assert!(matches!(
            v.kind,
            ReentrancyViolationKind::UninitializedLock { .. }
        ));

        let timeline = report.render_timeline();
        assert!(timeline.contains("[VIOLATION]"));
        assert!(timeline.contains("External call made while lock flag is Uninitialized"));
    }

    #[test]
    fn test_unlocked_external_call_flagged() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "withdraw")
            .set_lock("CVAULT", LockState::Unlocked) // Explicitly unlocked
            .external_call("CVAULT", "CTOKEN", "transfer")
            .exit_frame("CVAULT", "withdraw");

        let report = auditor.audit();
        assert_eq!(report.violations().len(), 1);
        assert!(matches!(
            report.violations()[0].kind,
            ReentrancyViolationKind::UnlockedExternalCall { .. }
        ));
    }

    #[test]
    fn test_dangling_lock_on_exit_flagged() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "deposit")
            .set_lock("CVAULT", LockState::Locked)
            // Flaw: Forgot to reset lock back to Unlocked upon frame exit!
            .exit_frame("CVAULT", "deposit");

        let report = auditor.audit();
        assert_eq!(report.violations().len(), 1);
        assert!(matches!(
            report.violations()[0].kind,
            ReentrancyViolationKind::DanglingLockOnExit { .. }
        ));
    }

    #[test]
    fn test_indirect_reentrancy_detection() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "withdraw")
            // External call without lock
            .external_call("CVAULT", "CATTACKER", "on_withdraw")
            .enter_frame("CATTACKER", "on_withdraw")
            // Malicious callback re-entering CVAULT
            .external_call("CATTACKER", "CVAULT", "withdraw")
            .enter_frame("CVAULT", "withdraw")
            .exit_frame("CVAULT", "withdraw")
            .exit_frame("CATTACKER", "on_withdraw")
            .exit_frame("CVAULT", "withdraw");

        let report = auditor.audit();
        assert!(report.violations().len() >= 2);

        // Should detect both uninitialized lock on external call and reentrancy attempt
        let has_uninit = report
            .violations()
            .iter()
            .any(|v| matches!(v.kind, ReentrancyViolationKind::UninitializedLock { .. }));
        let has_reentrancy = report
            .violations()
            .iter()
            .any(|v| matches!(v.kind, ReentrancyViolationKind::ReentrancyAttempt { .. }));

        assert!(has_uninit);
        assert!(has_reentrancy);

        let timeline = report.render_timeline();
        assert!(timeline.contains("Reentrant invocation"));
    }

    #[test]
    fn test_monitored_contract_filter() {
        let mut auditor = ReentrancyGuardAuditor::new().with_monitored_contract("CVAULT");

        auditor
            // CTOKEN makes external call without lock, but we only monitor CVAULT
            .enter_frame("CTOKEN", "mint")
            .external_call("CTOKEN", "COTHER", "notify")
            .exit_frame("CTOKEN", "mint")
            // CVAULT does it properly
            .enter_frame("CVAULT", "safe_fn")
            .set_lock("CVAULT", LockState::Locked)
            .external_call("CVAULT", "COTHER", "notify")
            .set_lock("CVAULT", LockState::Unlocked)
            .exit_frame("CVAULT", "safe_fn");

        let report = auditor.audit();
        assert!(report.is_safe());
    }

    #[test]
    fn test_premature_release_flagged() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "withdraw")
            .set_lock("CVAULT", LockState::Locked)
            .external_call("CVAULT", "CROUTER", "route")
            .enter_frame("CROUTER", "route")
            // Flaw: Lock is prematurely released while CROUTER frame is still executing!
            .set_lock("CVAULT", LockState::Unlocked)
            .exit_frame("CROUTER", "route")
            .exit_frame("CVAULT", "withdraw");

        let report = auditor.audit();
        assert!(!report.is_safe());
        let has_premature = report
            .violations()
            .iter()
            .any(|v| matches!(v.kind, ReentrancyViolationKind::PrematureRelease { .. }));
        assert!(has_premature);
        assert!(report
            .render_timeline()
            .contains("prematurely released while external call"));
    }

    #[test]
    #[should_panic(expected = "Reentrancy guard safety assertion failed")]
    fn test_assert_reentrancy_safe_panics_on_violation() {
        let mut auditor = ReentrancyGuardAuditor::new();
        auditor
            .enter_frame("CVAULT", "withdraw")
            .external_call("CVAULT", "CTOKEN", "transfer")
            .exit_frame("CVAULT", "withdraw");

        auditor.assert_reentrancy_safe();
    }

    #[contract]
    pub struct Vault;

    #[contractimpl]
    impl Vault {
        pub fn unsafe_withdraw(env: Env, attacker: Address) {
            emit_storage_read(&env, Symbol::new(&env, "balance"));
            let self_address = env.current_contract_address();
            MockReentrantAttackerClient::new(&env, &attacker)
                .attempt_reentry(&self_address, &Symbol::new(&env, "withdraw"));
        }

        pub fn safe_withdraw(env: Env, attacker: Address) {
            emit_storage_read(&env, Symbol::new(&env, "balance"));
            emit_storage_write(&env, Symbol::new(&env, "balance"));
            let self_address = env.current_contract_address();
            MockReentrantAttackerClient::new(&env, &attacker)
                .attempt_reentry(&self_address, &Symbol::new(&env, "withdraw"));
        }
    }

    fn detector_fixture() -> (Env, Address, Address) {
        let env = Env::default();
        let attacker = env.register(MockReentrantAttacker, ());
        let vault = env.register(Vault, ());
        (env, attacker, vault)
    }

    #[test]
    fn detector_accepts_a_safe_cei_sequence() {
        let env = Env::default();
        let mut detector = ReentrancyDetector::new(&env);
        detector
            .enter_frame("CVAULT", "withdraw")
            .record_storage_read("CVAULT", "balance")
            .record_storage_write("CVAULT", "balance")
            .record_external_call("CVAULT", "CTOKEN", "transfer")
            .exit_frame("CVAULT", "withdraw");

        assert!(!detector.has_cei_violations());
        assert!(!detector.has_violations());
        detector.assert_safe();
    }

    #[test]
    fn detector_flags_a_call_before_the_state_write() {
        let env = Env::default();
        let mut detector = ReentrancyDetector::new(&env);
        detector
            .enter_frame("CVAULT", "withdraw")
            .record_storage_read("CVAULT", "balance")
            .record_external_call("CVAULT", "CTOKEN", "transfer")
            .record_storage_write("CVAULT", "balance")
            .exit_frame("CVAULT", "withdraw");

        assert!(detector.has_cei_violations());
        assert!(!detector.has_reentrancy_attempts());
        assert_eq!(detector.violations().len(), 1);
        assert!(matches!(
            detector.violations()[0].kind,
            DetectorViolationKind::CeiViolation { .. }
        ));
    }

    #[test]
    fn detector_flags_reentry_into_an_active_frame() {
        let env = Env::default();
        let mut detector = ReentrancyDetector::new(&env);
        detector
            .enter_frame("CVAULT", "withdraw")
            .enter_frame("CATTACKER", "callback")
            .enter_frame("CVAULT", "withdraw")
            .exit_frame("CVAULT", "withdraw")
            .exit_frame("CATTACKER", "callback")
            .exit_frame("CVAULT", "withdraw");

        assert!(detector.has_reentrancy_attempts());
        assert!(!detector.has_cei_violations());
        assert!(detector.violations().iter().any(|violation| matches!(
            violation.kind,
            DetectorViolationKind::ReentrancyAttempt { depth: 3, .. }
        )));
    }

    #[test]
    fn report_names_the_label_and_the_violation() {
        let env = Env::default();
        let mut detector = ReentrancyDetector::new(&env);
        detector.label = Some("unsafe_withdraw".to_string());
        detector
            .enter_frame("CVAULT", "withdraw")
            .record_external_call("CVAULT", "CATTACKER", "callback")
            .exit_frame("CVAULT", "withdraw");

        let report = detector.report();
        assert!(report.contains("label=unsafe_withdraw"));
        assert!(report.contains("CEI violation"));
        assert!(report.contains("external call 'CATTACKER::callback'"));
        assert!(report.contains("Result: UNSAFE"));
    }

    #[test]
    fn monitor_observes_an_unsafe_instrumented_invocation() {
        let (env, attacker, vault) = detector_fixture();
        let mut detector = ReentrancyDetector::new(&env);

        let result = detector.monitor("unsafe_withdraw", || {
            VaultClient::new(&env, &vault).unsafe_withdraw(&attacker);
            7u32
        });

        assert_eq!(result, 7);
        assert!(detector.has_cei_violations(), "{}", detector.report());
        assert!(detector.has_reentrancy_attempts(), "{}", detector.report());
        assert!(detector.report().contains("label=unsafe_withdraw"));
    }

    #[test]
    fn monitor_separates_safe_state_order_from_the_attacker_attempt() {
        let (env, attacker, vault) = detector_fixture();
        let mut detector = ReentrancyDetector::new(&env);

        detector.monitor("safe_withdraw", || {
            VaultClient::new(&env, &vault).safe_withdraw(&attacker);
        });

        assert!(!detector.has_cei_violations(), "{}", detector.report());
        assert!(detector.has_reentrancy_attempts(), "{}", detector.report());
    }

    #[test]
    fn mock_attacker_event_reports_a_reentry_attempt() {
        let (env, attacker, vault) = detector_fixture();
        let vault_label = format!("{}", vault.to_string());
        let mut detector = ReentrancyDetector::new(&env);
        detector.enter_frame(vault_label.clone(), "withdraw");

        detector.monitor("mock_attacker", || {
            MockReentrantAttackerClient::new(&env, &attacker)
                .attack(&vault, &Symbol::new(&env, "withdraw"));
        });

        assert!(detector.has_reentrancy_attempts(), "{}", detector.report());
        assert!(detector.violations().iter().any(|violation| matches!(
            &violation.kind,
            DetectorViolationKind::ReentrancyAttempt { contract, .. }
                if contract == &vault_label
        )));
    }

    #[test]
    #[should_panic(expected = "Reentrancy safety assertion failed")]
    fn detector_assert_safe_panics_on_a_violation() {
        let env = Env::default();
        let mut detector = ReentrancyDetector::new(&env);
        detector
            .enter_frame("CVAULT", "withdraw")
            .record_external_call("CVAULT", "CTOKEN", "transfer")
            .exit_frame("CVAULT", "withdraw");
        detector.assert_safe();
    }

    #[test]
    fn repeated_monitors_only_observe_new_events() {
        let (env, attacker, vault) = detector_fixture();
        let mut detector = ReentrancyDetector::new(&env);

        detector.monitor("first", || {
            VaultClient::new(&env, &vault).safe_withdraw(&attacker);
        });
        assert_eq!(detector.violations().len(), 1);

        detector.monitor("second", || {
            VaultClient::new(&env, &vault).safe_withdraw(&attacker);
        });
        assert_eq!(detector.violations().len(), 2);
        assert_eq!(
            detector
                .violations()
                .iter()
                .filter(|violation| violation.label.as_deref() == Some("second"))
                .count(),
            1
        );
        assert!(!detector.has_cei_violations());
    }
}
