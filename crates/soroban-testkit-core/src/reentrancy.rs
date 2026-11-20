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

use soroban_env_host::xdr::{ContractEventBody, ContractEventType, ScVal};
use soroban_sdk::{
    xdr::{ContractId, Hash, ScAddress},
    Address, Env, TryFromVal,
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
}
