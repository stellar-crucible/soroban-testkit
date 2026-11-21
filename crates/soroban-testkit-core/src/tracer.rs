//! Tracing the cross-contract call tree of an invocation.
//!
//! A router that calls a pair that calls two tokens fails three frames down,
//! and the panic a test sees names only the outermost call. [`CallTracer`]
//! recovers the whole tree after the fact: every frame's depth, contract,
//! function, the CPU instructions and memory metered while it ran, and whether
//! it returned or failed.
//!
//! ```rust
//! use soroban_sdk::{contract, contractimpl, Address, Env, IntoVal, Symbol};
//! use soroban_testkit_core::tracer::CallTracer;
//!
//! #[contract]
//! pub struct Leaf;
//!
//! #[contractimpl]
//! impl Leaf {
//!     pub fn ping(_env: Env) -> u32 {
//!         1
//!     }
//! }
//!
//! #[contract]
//! pub struct Caller;
//!
//! #[contractimpl]
//! impl Caller {
//!     pub fn call(env: Env, leaf: Address) -> u32 {
//!         env.invoke_contract(&leaf, &Symbol::new(&env, "ping"), ().into_val(&env))
//!     }
//! }
//!
//! # fn main() {
//! let env = Env::default();
//! let leaf = env.register(Leaf, ());
//! let caller = env.register(Caller, ());
//!
//! let trace = CallTracer::capture(&env, || CallerClient::new(&env, &caller).call(&leaf))
//!     .with_label(&caller, "caller")
//!     .with_label(&leaf, "leaf");
//!
//! assert_eq!(trace.depth(), 2);
//! trace.assert_success();
//! println!("{}", trace.render_ascii());
//! println!("{}", trace.render_mermaid());
//! # }
//! ```
//!
//! # Where the data comes from
//!
//! Nothing is hooked while a test runs, so a test that never builds a tracer
//! pays nothing for it. After the closure returns, the tracer reads two
//! records the SDK's test host already keeps for the most recent top-level
//! invocation:
//!
//! * the host's per-invocation resource meter
//!   (`Host::get_detailed_last_invocation_resources`), which is the call tree
//!   itself — one node per contract frame with the instructions and memory
//!   metered inside it, sub-calls included;
//! * the `fn_call` / `fn_return` / `error` diagnostic events the host emits at
//!   debug level (the SDK's default in tests), which say which frames returned
//!   and which error a failing frame raised.
//!
//! Both records are reset at the start of every top-level invocation, so
//! [`CallTracer::capture`] describes the *last* top-level call its closure
//! makes. Use [`CallTracer::record`] once per call to trace several.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::panic::{self, AssertUnwindSafe};

use soroban_env_host::xdr::{
    AccountId, ContractEventBody, ContractEventType, ContractId, Hash, PublicKey, ScAddress,
    ScError, ScVal, Uint256,
};
use soroban_sdk::{Address, Env};

/// How a traced frame ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallOutcome {
    /// The frame returned a value.
    Success,
    /// The frame failed. The error is the first one the host reported while
    /// the frame was on top of the stack, when one was reported.
    Error(Option<ScError>),
    /// The host emitted no diagnostic events (diagnostics were switched off),
    /// so whether the frame returned is not known.
    Unknown,
}

impl CallOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, CallOutcome::Success)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, CallOutcome::Error(_))
    }
}

impl fmt::Display for CallOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallOutcome::Success => f.write_str("ok"),
            CallOutcome::Error(Some(error)) => write!(f, "error: {}", render_sc_error(error)),
            CallOutcome::Error(None) => f.write_str("error"),
            CallOutcome::Unknown => f.write_str("unknown"),
        }
    }
}

/// One contract frame of a traced invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceFrame {
    /// `1` for the top-level call, `2` for what it calls, and so on.
    pub depth: u32,
    /// The contract the frame ran in.
    pub contract: ScAddress,
    /// The function the frame invoked.
    pub function: String,
    /// CPU instructions metered while the frame ran, sub-calls included.
    pub cpu_insns: u64,
    /// Memory bytes metered while the frame ran, sub-calls included.
    pub mem_bytes: u64,
    /// Whether the frame returned.
    pub outcome: CallOutcome,
    /// Calls the frame made, in the order it made them.
    pub children: Vec<TraceFrame>,
}

impl TraceFrame {
    /// CPU instructions attributed to this frame alone, without its sub-calls.
    pub fn self_cpu_insns(&self) -> u64 {
        let children: u64 = self.children.iter().map(|c| c.cpu_insns).sum();
        self.cpu_insns.saturating_sub(children)
    }

    /// Memory bytes attributed to this frame alone, without its sub-calls.
    pub fn self_mem_bytes(&self) -> u64 {
        let children: u64 = self.children.iter().map(|c| c.mem_bytes).sum();
        self.mem_bytes.saturating_sub(children)
    }

    /// Whether the frame invoked `function` on `contract`.
    pub fn is_call_to(&self, contract: &Address, function: &str) -> bool {
        self.contract == ScAddress::from(contract) && self.function == function
    }

    /// The deepest level reached from this frame.
    pub fn max_depth(&self) -> u32 {
        self.children
            .iter()
            .map(TraceFrame::max_depth)
            .max()
            .unwrap_or(self.depth)
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a TraceFrame>) {
        out.push(self);
        for child in &self.children {
            child.walk(out);
        }
    }

    /// The path to the deepest failing frame under this one, this frame first.
    fn failure_path<'a>(&'a self, path: &mut Vec<&'a TraceFrame>) -> bool {
        if !self.outcome.is_error() {
            return false;
        }
        path.push(self);
        // A failing frame usually failed because a child did; follow the last
        // failing child, the one whose error the frame propagated.
        if let Some(child) = self.children.iter().rev().find(|c| c.outcome.is_error()) {
            child.failure_path(path);
        }
        true
    }
}

/// The call tree of one or more top-level invocations.
///
/// See the [module documentation](self) for how the tree is recovered.
#[derive(Debug, Clone, Default)]
pub struct CallTracer {
    roots: Vec<TraceFrame>,
    labels: BTreeMap<ScAddress, String>,
    panics: Vec<String>,
}

impl CallTracer {
    /// An empty tracer, for recording several invocations with
    /// [`CallTracer::record`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `invocation` and trace the last top-level contract call it made.
    ///
    /// A panic inside the closure — a client call whose contract failed — is
    /// caught and kept, so the trace of the failing call can still be read;
    /// [`CallTracer::assert_success`] turns it back into a test failure with
    /// the invocation path attached.
    pub fn capture<T>(env: &Env, invocation: impl FnOnce() -> T) -> Self {
        let mut tracer = Self::new();
        tracer.record(env, invocation);
        tracer
    }

    /// Run `invocation` and append the trace of the last top-level call it
    /// made, returning the closure's value when it did not panic.
    pub fn record<T>(&mut self, env: &Env, invocation: impl FnOnce() -> T) -> Option<T> {
        let before = env.host().get_detailed_last_invocation_resources();
        let result = panic::catch_unwind(AssertUnwindSafe(invocation));
        let after = env.host().get_detailed_last_invocation_resources();

        let value = match result {
            Ok(value) => Some(value),
            Err(payload) => {
                self.panics.push(panic_text(payload.as_ref()));
                None
            }
        };

        // The meter keeps the previous invocation when the closure made none;
        // a reading that did not change is not a new call.
        let Some(resources) = after.filter(|after| before.as_ref() != Some(after)) else {
            return value;
        };

        // The host does not export the meter's types by name, so the tree is
        // flattened here, where their types are inferred.
        let mut metered: Vec<MeteredCall> = Vec::new();
        let mut stack = vec![(&resources, 1u32, None::<usize>)];
        while let Some((node, depth, parent)) = stack.pop() {
            let Some((contract, function)) = decode_invocation(&format!("{:?}", node.invocation))
            else {
                // A Wasm upload or contract creation entry point: not a call.
                continue;
            };
            metered.push(MeteredCall {
                depth,
                parent,
                contract,
                function,
                cpu_insns: node.resources.instructions.max(0) as u64,
                mem_bytes: node.resources.mem_bytes.max(0) as u64,
            });
            let index = metered.len() - 1;
            for child in node.sub_call_resources.iter().rev() {
                stack.push((child, depth + 1, Some(index)));
            }
        }

        if let Some(root) = build_tree(metered, diagnostic_calls(env).as_deref()) {
            self.roots.push(root);
        }
        value
    }

    /// Name `contract` in rendered output instead of its abbreviated strkey.
    pub fn with_label(mut self, contract: &Address, label: impl Into<String>) -> Self {
        self.label(contract, label);
        self
    }

    /// Name `contract` in rendered output instead of its abbreviated strkey.
    pub fn label(&mut self, contract: &Address, label: impl Into<String>) -> &mut Self {
        self.labels.insert(ScAddress::from(contract), label.into());
        self
    }

    /// The traced top-level calls, oldest first.
    pub fn roots(&self) -> &[TraceFrame] {
        &self.roots
    }

    /// The most recently traced top-level call.
    pub fn root(&self) -> Option<&TraceFrame> {
        self.roots.last()
    }

    /// Every frame, depth first, in the order the calls were made.
    pub fn frames(&self) -> Vec<&TraceFrame> {
        let mut out = Vec::new();
        for root in &self.roots {
            root.walk(&mut out);
        }
        out
    }

    /// How many contract frames were traced.
    pub fn call_count(&self) -> usize {
        self.frames().len()
    }

    /// The deepest nesting reached: `1` for a call that made no sub-calls,
    /// `0` when nothing was traced.
    pub fn depth(&self) -> u32 {
        self.roots
            .iter()
            .map(TraceFrame::max_depth)
            .max()
            .unwrap_or(0)
    }

    /// Frames that invoked `function` on `contract`.
    pub fn calls_to(&self, contract: &Address, function: &str) -> Vec<&TraceFrame> {
        self.frames()
            .into_iter()
            .filter(|frame| frame.is_call_to(contract, function))
            .collect()
    }

    /// Panic messages the traced closures raised.
    pub fn panics(&self) -> &[String] {
        &self.panics
    }

    /// Whether every traced frame returned and no closure panicked.
    pub fn succeeded(&self) -> bool {
        self.panics.is_empty() && self.frames().iter().all(|f| !f.outcome.is_error())
    }

    /// The path from a top-level call down to the deepest frame that failed,
    /// or `None` when no frame failed.
    pub fn failure_path(&self) -> Option<Vec<&TraceFrame>> {
        self.roots.iter().rev().find_map(|root| {
            let mut path = Vec::new();
            root.failure_path(&mut path).then_some(path)
        })
    }

    /// `router::swap -> pair::swap -> token::transfer`, the call path that led
    /// to the failure, or `None` when no frame failed.
    pub fn render_failure_path(&self) -> Option<String> {
        let path = self.failure_path()?;
        let calls: Vec<String> = path.iter().map(|frame| self.call_name(frame)).collect();
        let outcome = &path.last()?.outcome;
        Some(format!("{} ({outcome})", calls.join(" -> ")))
    }

    /// Fail unless every traced frame returned, printing the invocation path
    /// to the failure and the whole call tree.
    #[track_caller]
    pub fn assert_success(&self) {
        if self.succeeded() {
            return;
        }
        let mut message = String::from("CallTracer: the traced invocation failed\n");
        if let Some(path) = self.render_failure_path() {
            let _ = writeln!(message, "  failing path: {path}");
        }
        for text in &self.panics {
            let _ = writeln!(message, "  panic: {text}");
        }
        let _ = write!(message, "  call tree:\n{}", indent(&self.render_ascii(), 4));
        panic!("{message}");
    }

    /// Fail unless the trace reached exactly `depth` levels.
    #[track_caller]
    pub fn assert_depth(&self, depth: u32) {
        assert_eq!(
            self.depth(),
            depth,
            "CallTracer: expected a call depth of {depth}, traced {}\n{}",
            self.depth(),
            self.render_ascii()
        );
    }

    /// The call tree as indented text, one frame per line:
    ///
    /// ```text
    /// router::swap_exact_in [ok] cpu=271623 mem=51987
    /// └── pool::swap [ok] cpu=242342 mem=42756
    ///     ├── token_b::transfer [ok] cpu=74519 mem=13436
    ///     └── token_a::transfer [ok] cpu=75818 mem=13131
    /// ```
    pub fn render_ascii(&self) -> String {
        let mut out = String::new();
        if self.roots.is_empty() {
            out.push_str("(no contract invocation traced)\n");
        }
        for root in &self.roots {
            let _ = writeln!(out, "{}", self.frame_line(root));
            self.ascii_children(root, "", &mut out);
        }
        out
    }

    fn ascii_children(&self, frame: &TraceFrame, prefix: &str, out: &mut String) {
        let last = frame.children.len().saturating_sub(1);
        for (index, child) in frame.children.iter().enumerate() {
            let (branch, carry) = if index == last {
                ("└── ", "    ")
            } else {
                ("├── ", "│   ")
            };
            let _ = writeln!(out, "{prefix}{branch}{}", self.frame_line(child));
            self.ascii_children(child, &format!("{prefix}{carry}"), out);
        }
    }

    fn frame_line(&self, frame: &TraceFrame) -> String {
        format!(
            "{} [{}] cpu={} mem={}",
            self.call_name(frame),
            frame.outcome,
            frame.cpu_insns,
            frame.mem_bytes
        )
    }

    /// The call tree as a Mermaid sequence diagram, ready to paste into a
    /// Markdown file or a pull request.
    pub fn render_mermaid(&self) -> String {
        let mut participants: Vec<&ScAddress> = Vec::new();
        for frame in self.frames() {
            if !participants.contains(&&frame.contract) {
                participants.push(&frame.contract);
            }
        }
        let alias = |contract: &ScAddress| {
            let index = participants
                .iter()
                .position(|p| *p == contract)
                .unwrap_or(0);
            format!("C{index}")
        };

        let mut out = String::from("sequenceDiagram\n    participant Test\n");
        for contract in &participants {
            let _ = writeln!(
                out,
                "    participant {} as {}",
                alias(contract),
                mermaid_text(&self.contract_name(contract))
            );
        }
        for root in &self.roots {
            self.mermaid_frame(root, "Test", &alias, &mut out);
        }
        out
    }

    fn mermaid_frame(
        &self,
        frame: &TraceFrame,
        caller: &str,
        alias: &dyn Fn(&ScAddress) -> String,
        out: &mut String,
    ) {
        let callee = alias(&frame.contract);
        let _ = writeln!(
            out,
            "    {caller}->>{callee}: {} (cpu={}, mem={})",
            mermaid_text(&frame.function),
            frame.cpu_insns,
            frame.mem_bytes
        );
        for child in &frame.children {
            self.mermaid_frame(child, &callee, alias, out);
        }
        let arrow = if frame.outcome.is_error() {
            "--x"
        } else {
            "-->>"
        };
        let _ = writeln!(
            out,
            "    {callee}{arrow}{caller}: {}",
            mermaid_text(&frame.outcome.to_string())
        );
    }

    /// `label::function`, or `CABC…WXYZ::function` for an unlabelled contract.
    pub fn call_name(&self, frame: &TraceFrame) -> String {
        format!(
            "{}::{}",
            self.contract_name(&frame.contract),
            frame.function
        )
    }

    fn contract_name(&self, contract: &ScAddress) -> String {
        if let Some(label) = self.labels.get(contract) {
            return label.clone();
        }
        let strkey = contract.to_string();
        if strkey.len() > 12 {
            format!("{}…{}", &strkey[..4], &strkey[strkey.len() - 4..])
        } else {
            strkey
        }
    }
}

impl fmt::Display for CallTracer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render_ascii())
    }
}

/// A call the diagnostic events recorded, in the order the calls were made.
#[derive(Debug, Clone)]
struct DiagnosticCall {
    contract: ScAddress,
    function: String,
    returned: bool,
    error: Option<ScError>,
}

/// Replay the debug-level diagnostic events of the last invocation into the
/// calls they describe. `None` when the host emitted no `fn_call` at all.
fn diagnostic_calls(env: &Env) -> Option<Vec<DiagnosticCall>> {
    let events = env.host().get_diagnostic_events().ok()?;
    let mut calls: Vec<DiagnosticCall> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();

    // Pop a frame that never returned, handing its error to its caller: the
    // caller either caught it (and its own fn_return overrides the error) or
    // failed with it.
    fn pop_failed(calls: &mut [DiagnosticCall], stack: &mut Vec<usize>) {
        if let Some(index) = stack.pop() {
            let error = calls[index].error.clone();
            if let (Some(parent), Some(error)) = (stack.last(), error) {
                calls[*parent].error.get_or_insert(error);
            }
        }
    }

    for event in events.0.iter() {
        if event.event.type_ != ContractEventType::Diagnostic {
            continue;
        }
        let ContractEventBody::V0(body) = &event.event.body;
        let topics = body.topics.as_slice();
        let Some(ScVal::Symbol(head)) = topics.first() else {
            continue;
        };
        match head.to_utf8_string_lossy().as_str() {
            "fn_call" => {
                let (Some(ScVal::Bytes(id)), Some(ScVal::Symbol(function))) =
                    (topics.get(1), topics.get(2))
                else {
                    continue;
                };
                let Ok(hash) = <[u8; 32]>::try_from(id.as_slice()) else {
                    continue;
                };
                let caller = event
                    .event
                    .contract_id
                    .as_ref()
                    .map(|id| ScAddress::Contract(id.clone()));
                while let Some(top) = stack.last() {
                    if Some(&calls[*top].contract) == caller.as_ref() {
                        break;
                    }
                    pop_failed(&mut calls, &mut stack);
                }
                calls.push(DiagnosticCall {
                    contract: ScAddress::Contract(ContractId(Hash(hash))),
                    function: function.to_utf8_string_lossy(),
                    returned: false,
                    error: None,
                });
                stack.push(calls.len() - 1);
            }
            "fn_return" => {
                let Some(ScVal::Symbol(function)) = topics.get(1) else {
                    continue;
                };
                let function = function.to_utf8_string_lossy();
                let contract = event
                    .event
                    .contract_id
                    .as_ref()
                    .map(|id| ScAddress::Contract(id.clone()));
                let matches = |call: &DiagnosticCall| {
                    call.function == function && Some(&call.contract) == contract.as_ref()
                };
                if !stack.iter().any(|index| matches(&calls[*index])) {
                    continue;
                }
                while let Some(top) = stack.last().copied() {
                    if matches(&calls[top]) {
                        calls[top].returned = true;
                        stack.pop();
                        break;
                    }
                    pop_failed(&mut calls, &mut stack);
                }
            }
            "error" => {
                if let (Some(top), Some(ScVal::Error(error))) = (stack.last(), topics.get(1)) {
                    calls[*top].error.get_or_insert(error.clone());
                }
            }
            _ => {}
        }
    }

    (!calls.is_empty()).then_some(calls)
}

/// One node of the host's invocation meter, flattened depth first.
struct MeteredCall {
    depth: u32,
    parent: Option<usize>,
    contract: ScAddress,
    function: String,
    cpu_insns: u64,
    mem_bytes: u64,
}

/// Read `InvokeContract(<address>, ScSymbol(StringM(<name>)))`, the `Debug`
/// form of the host's metering record for a contract call.
fn decode_invocation(debug: &str) -> Option<(ScAddress, String)> {
    let body = debug.strip_prefix("InvokeContract(")?;
    let function = body
        .rsplit_once("ScSymbol(StringM(")?
        .1
        .trim_end_matches(')')
        .to_string();
    let hex_after = |marker: &str| -> Option<[u8; 32]> {
        let start = body.find(marker)? + marker.len();
        let hex = body.get(start..start + 64)?;
        let mut bytes = [0u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
        }
        Some(bytes)
    };
    let contract = if body.starts_with("Contract(") {
        ScAddress::Contract(ContractId(Hash(hex_after("Hash(")?)))
    } else if body.starts_with("Account(") {
        ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
            hex_after("Uint256(")?,
        ))))
    } else {
        return None;
    };
    Some((contract, function))
}

/// Assemble the flattened meter records into a tree, matching each against
/// the diagnostic calls in order. A metered frame with no matching `fn_call`
/// was rejected before it started (re-entry, a missing contract) and inherits
/// its caller's error.
fn build_tree(metered: Vec<MeteredCall>, calls: Option<&[DiagnosticCall]>) -> Option<TraceFrame> {
    let mut cursor = 0;
    let mut errors: Vec<Option<ScError>> = Vec::with_capacity(metered.len());
    let mut frames: Vec<(Option<usize>, TraceFrame)> = Vec::with_capacity(metered.len());

    for call in metered {
        let parent_error = call.parent.and_then(|p| errors.get(p).cloned().flatten());
        let (outcome, own_error) = match calls {
            None => (CallOutcome::Unknown, None),
            Some(diagnostics) => match diagnostics.get(cursor) {
                Some(seen) if seen.contract == call.contract && seen.function == call.function => {
                    cursor += 1;
                    if seen.returned {
                        (CallOutcome::Success, None)
                    } else {
                        (CallOutcome::Error(seen.error.clone()), seen.error.clone())
                    }
                }
                _ => (CallOutcome::Error(parent_error.clone()), parent_error),
            },
        };
        errors.push(own_error);
        frames.push((
            call.parent,
            TraceFrame {
                depth: call.depth,
                contract: call.contract,
                function: call.function,
                cpu_insns: call.cpu_insns,
                mem_bytes: call.mem_bytes,
                outcome,
                children: Vec::new(),
            },
        ));
    }

    // Children come after their parents, so folding from the end attaches
    // every frame before its parent is itself attached. Reversing keeps the
    // order the calls were made in.
    while frames.len() > 1 {
        let (parent, frame) = frames.pop()?;
        let parent = parent?;
        frames[parent].1.children.insert(0, frame);
    }
    frames.pop().map(|(_, root)| root)
}

fn render_sc_error(error: &ScError) -> String {
    match error {
        ScError::Contract(code) => format!("Contract({code})"),
        other => format!("{other:?}"),
    }
}

fn mermaid_text(text: &str) -> String {
    text.replace(['\n', ';', '#'], " ")
}

fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines().map(|line| format!("{pad}{line}\n")).collect()
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contracterror, contractimpl, panic_with_error, symbol_short, vec, Address, Env,
        IntoVal, Symbol, Val, Vec,
    };

    #[contracterror]
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    #[repr(u32)]
    pub enum LeafError {
        Refused = 7,
    }

    /// The bottom of every chain: does a little storage work and returns.
    #[contract]
    pub struct Leaf;

    #[contractimpl]
    impl Leaf {
        pub fn work(env: Env, n: u32) -> u32 {
            env.storage().instance().set(&symbol_short!("n"), &n);
            n + 1
        }

        pub fn refuse(env: Env) -> u32 {
            panic_with_error!(&env, LeafError::Refused)
        }
    }

    /// Forwards to the next contract in the chain.
    #[contract]
    pub struct Hop;

    #[contractimpl]
    impl Hop {
        pub fn forward(env: Env, next: Address, function: Symbol, args: Vec<Val>) -> u32 {
            env.invoke_contract(&next, &function, args)
        }

        /// Calls `leaf.work` twice, so the frame has two children.
        pub fn fan_out(env: Env, leaf: Address) -> u32 {
            let work = Symbol::new(&env, "work");
            let a: u32 = env.invoke_contract(&leaf, &work, vec![&env, 1u32.into_val(&env)]);
            let b: u32 = env.invoke_contract(&leaf, &work, vec![&env, 2u32.into_val(&env)]);
            a + b
        }

        /// Calls `leaf.refuse` with `try_invoke_contract` and recovers.
        pub fn recover(env: Env, leaf: Address) -> u32 {
            let result = env.try_invoke_contract::<u32, LeafError>(
                &leaf,
                &Symbol::new(&env, "refuse"),
                Vec::new(&env),
            );
            match result {
                Ok(Ok(value)) => value,
                _ => 0,
            }
        }

        /// Calls back into `back.poke`, which calls this contract again.
        pub fn reenter(env: Env, back: Address) -> u32 {
            let me = env.current_contract_address();
            env.invoke_contract(
                &back,
                &Symbol::new(&env, "poke"),
                vec![&env, me.into_val(&env)],
            )
        }

        pub fn target(_env: Env) -> u32 {
            99
        }
    }

    /// Calls `target` on whatever address it is given.
    #[contract]
    pub struct Bouncer;

    #[contractimpl]
    impl Bouncer {
        pub fn poke(env: Env, origin: Address) -> u32 {
            env.invoke_contract(&origin, &Symbol::new(&env, "target"), Vec::new(&env))
        }
    }

    struct Chain {
        env: Env,
        top: Address,
        middle: Address,
        leaf: Address,
    }

    fn chain() -> Chain {
        let env = Env::default();
        let leaf = env.register(Leaf, ());
        let middle = env.register(Hop, ());
        let top = env.register(Hop, ());
        Chain {
            env,
            top,
            middle,
            leaf,
        }
    }

    fn work_args(env: &Env, n: u32) -> Vec<Val> {
        vec![env, n.into_val(env)]
    }

    /// top.forward -> middle.forward -> leaf.<function>
    fn call_three_deep(chain: &Chain, function: &str) -> u32 {
        let env = &chain.env;
        let inner_args: Vec<Val> = if function == "work" {
            work_args(env, 5)
        } else {
            Vec::new(env)
        };
        let middle_args: Vec<Val> = vec![
            env,
            chain.leaf.into_val(env),
            Symbol::new(env, function).into_val(env),
            inner_args.into_val(env),
        ];
        HopClient::new(env, &chain.top).forward(
            &chain.middle,
            &Symbol::new(env, "forward"),
            &middle_args,
        )
    }

    fn labelled(tracer: CallTracer, chain: &Chain) -> CallTracer {
        tracer
            .with_label(&chain.top, "top")
            .with_label(&chain.middle, "middle")
            .with_label(&chain.leaf, "leaf")
    }

    #[test]
    fn a_direct_call_traces_one_frame() {
        let chain = chain();
        let env = &chain.env;

        let trace = CallTracer::capture(env, || LeafClient::new(env, &chain.leaf).work(&1));

        assert_eq!(trace.depth(), 1);
        assert_eq!(trace.call_count(), 1);
        let root = trace.root().unwrap();
        assert!(root.is_call_to(&chain.leaf, "work"));
        assert_eq!(root.outcome, CallOutcome::Success);
        assert!(root.cpu_insns > 0);
        assert!(root.mem_bytes > 0);
        trace.assert_success();
    }

    #[test]
    fn a_three_deep_chain_is_traced_frame_by_frame() {
        let chain = chain();
        let env = &chain.env;

        let trace = labelled(
            CallTracer::capture(env, || call_three_deep(&chain, "work")),
            &chain,
        );

        trace.assert_depth(3);
        trace.assert_success();
        let frames = trace.frames();
        assert_eq!(frames.len(), 3);
        assert!(frames[0].is_call_to(&chain.top, "forward"));
        assert!(frames[1].is_call_to(&chain.middle, "forward"));
        assert!(frames[2].is_call_to(&chain.leaf, "work"));
        assert_eq!(
            frames.iter().map(|f| f.depth).collect::<std::vec::Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn per_frame_costs_nest_inside_their_callers() {
        let chain = chain();
        let env = &chain.env;

        let trace = CallTracer::capture(env, || call_three_deep(&chain, "work"));
        let frames = trace.frames();

        for pair in frames.windows(2) {
            assert!(
                pair[0].cpu_insns >= pair[1].cpu_insns,
                "a caller's inclusive cost covers its callee"
            );
            assert!(pair[0].mem_bytes >= pair[1].mem_bytes);
        }
        let leaf = frames[2];
        assert_eq!(
            leaf.self_cpu_insns(),
            leaf.cpu_insns,
            "a leaf has no sub-calls"
        );
        assert!(frames[0].self_cpu_insns() < frames[0].cpu_insns);
        assert!(frames[0].self_mem_bytes() <= frames[0].mem_bytes);
    }

    #[test]
    fn a_frame_with_two_calls_has_two_children_in_order() {
        let chain = chain();
        let env = &chain.env;

        let trace =
            CallTracer::capture(env, || HopClient::new(env, &chain.top).fan_out(&chain.leaf));

        trace.assert_depth(2);
        let root = trace.root().unwrap();
        assert_eq!(root.children.len(), 2);
        assert_eq!(trace.calls_to(&chain.leaf, "work").len(), 2);
        trace.assert_success();
    }

    #[test]
    fn a_failure_three_frames_down_is_located() {
        let chain = chain();
        let env = &chain.env;

        let trace = labelled(
            CallTracer::capture(env, || call_three_deep(&chain, "refuse")),
            &chain,
        );

        assert!(!trace.succeeded());
        assert_eq!(trace.panics().len(), 1, "the client call panicked");
        let path = trace.failure_path().expect("a frame failed");
        assert_eq!(path.len(), 3);
        assert!(path[2].is_call_to(&chain.leaf, "refuse"));
        assert_eq!(
            path[2].outcome,
            CallOutcome::Error(Some(ScError::Contract(7)))
        );
        assert_eq!(
            trace.render_failure_path().unwrap(),
            "top::forward -> middle::forward -> leaf::refuse (error: Contract(7))"
        );
    }

    #[test]
    #[should_panic(expected = "failing path: top::forward -> middle::forward -> leaf::refuse")]
    fn assert_success_prints_the_failing_path() {
        let chain = chain();
        let env = &chain.env;

        labelled(
            CallTracer::capture(env, || call_three_deep(&chain, "refuse")),
            &chain,
        )
        .assert_success();
    }

    #[test]
    fn a_caught_sub_call_failure_leaves_the_caller_successful() {
        let chain = chain();
        let env = &chain.env;

        let trace =
            CallTracer::capture(env, || HopClient::new(env, &chain.top).recover(&chain.leaf));

        let root = trace.root().unwrap();
        assert_eq!(root.outcome, CallOutcome::Success);
        assert_eq!(root.children.len(), 1);
        assert!(root.children[0].outcome.is_error());
        assert!(trace.panics().is_empty());
        assert!(!trace.succeeded(), "a frame in the tree failed");
    }

    #[test]
    fn a_reentry_attempt_is_traced_as_a_rejected_frame() {
        let env = Env::default();
        let origin = env.register(Hop, ());
        let bouncer = env.register(Bouncer, ());

        let trace = CallTracer::capture(&env, || HopClient::new(&env, &origin).reenter(&bouncer))
            .with_label(&origin, "origin")
            .with_label(&bouncer, "bouncer");

        trace.assert_depth(3);
        let frames = trace.frames();
        assert!(frames[0].is_call_to(&origin, "reenter"));
        assert!(frames[1].is_call_to(&bouncer, "poke"));
        assert!(frames[2].is_call_to(&origin, "target"));
        assert!(frames.iter().all(|f| f.outcome.is_error()));
        let path = trace.render_failure_path().unwrap();
        assert!(
            path.starts_with("origin::reenter -> bouncer::poke -> origin::target"),
            "{path}"
        );
    }

    #[test]
    fn record_appends_one_root_per_invocation() {
        let chain = chain();
        let env = &chain.env;
        let leaf = LeafClient::new(env, &chain.leaf);

        let mut tracer = CallTracer::new();
        assert_eq!(tracer.record(env, || leaf.work(&1)), Some(2));
        tracer.record(env, || call_three_deep(&chain, "work"));

        assert_eq!(tracer.roots().len(), 2);
        assert_eq!(tracer.depth(), 3);
        assert_eq!(tracer.call_count(), 4);
    }

    #[test]
    fn a_closure_that_calls_nothing_traces_nothing() {
        let chain = chain();
        let env = &chain.env;
        LeafClient::new(env, &chain.leaf).work(&1);

        let trace = CallTracer::capture(env, || 1 + 1);

        assert_eq!(trace.depth(), 0);
        assert!(trace.roots().is_empty());
        assert!(trace
            .render_ascii()
            .contains("no contract invocation traced"));
    }

    #[test]
    fn render_ascii_draws_the_tree() {
        let chain = chain();
        let env = &chain.env;

        let trace = labelled(
            CallTracer::capture(env, || call_three_deep(&chain, "work")),
            &chain,
        );
        let ascii = trace.render_ascii();
        let lines: std::vec::Vec<&str> = ascii.lines().collect();

        assert_eq!(lines.len(), 3, "{ascii}");
        assert!(lines[0].starts_with("top::forward [ok] cpu="), "{ascii}");
        assert!(lines[1].starts_with("└── middle::forward [ok]"), "{ascii}");
        assert!(lines[2].starts_with("    └── leaf::work [ok]"), "{ascii}");
    }

    #[test]
    fn render_ascii_uses_tees_between_siblings() {
        let chain = chain();
        let env = &chain.env;

        let trace =
            CallTracer::capture(env, || HopClient::new(env, &chain.top).fan_out(&chain.leaf))
                .with_label(&chain.top, "hop")
                .with_label(&chain.leaf, "leaf");
        let ascii = trace.render_ascii();

        assert!(ascii.contains("├── leaf::work"), "{ascii}");
        assert!(ascii.contains("└── leaf::work"), "{ascii}");
    }

    #[test]
    fn render_mermaid_draws_a_sequence_diagram() {
        let chain = chain();
        let env = &chain.env;

        let trace = labelled(
            CallTracer::capture(env, || call_three_deep(&chain, "refuse")),
            &chain,
        );
        let mermaid = trace.render_mermaid();

        assert!(mermaid.starts_with("sequenceDiagram\n    participant Test\n"));
        assert!(mermaid.contains("participant C0 as top"));
        assert!(mermaid.contains("participant C1 as middle"));
        assert!(mermaid.contains("participant C2 as leaf"));
        assert!(mermaid.contains("Test->>C0: forward"));
        assert!(mermaid.contains("C1->>C2: refuse"));
        assert!(mermaid.contains("C2--xC1: error: Contract(7)"), "{mermaid}");
    }

    #[test]
    fn unlabelled_contracts_render_as_abbreviated_strkeys() {
        let chain = chain();
        let env = &chain.env;

        let trace = CallTracer::capture(env, || LeafClient::new(env, &chain.leaf).work(&1));
        let name = trace.call_name(trace.root().unwrap());

        assert!(name.starts_with('C'), "{name}");
        assert!(name.contains('…'), "{name}");
        assert!(name.ends_with("::work"), "{name}");
    }
}
