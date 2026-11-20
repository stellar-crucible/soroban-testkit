//! Contract call-stack depth auditing.
//!
//! Soroban limits how deeply one contract call may nest inside another. The
//! pinned host (`soroban-env-host` 28.0.2) defines that ceiling as
//! [`DEFAULT_HOST_DEPTH_LIMIT`] — `100` — in `src/budget/limits.rs`, and
//! `Host::with_frame` in `src/host/frame.rs` refuses to open a frame when the
//! context stack has already reached it, returning
//! `Context/ExceededLimit`. With an empty context stack — the case for a test
//! environment's top-level client call — contract frames 1 through 100 are
//! accepted and the 101st nested call is the one that fails. An entry point
//! reached through `Host::invoke_function` pushes a host-function frame first,
//! so it has room for 99 contract frames.
//!
//! [`StackDepthAuditor`] reconstructs every frame of a call chain from the
//! diagnostic events the host records for each contract call: `fn_call`
//! carries the called contract id and function name, and `fn_return` closes
//! the frame. It reports the high-water depth, warns before the host limit is
//! reached, and can emulate a lower ceiling so a contract's recovery path can
//! be exercised without actually running 100 nested calls.
//!
//! ```rust
//! use soroban_testkit_core::StackDepthAuditor;
//!
//! let mut auditor = StackDepthAuditor::new();
//! auditor
//!     .enter("CDEPOSIT", "deposit")
//!     .enter("CTOKEN", "transfer");
//!
//! assert_eq!(auditor.max_depth(), 2);
//! assert!(auditor.trace().contains("depth=2"));
//! assert!(!auditor.warning_triggered());
//! ```
//!
//! To read a chain that already ran, observe the environment afterwards:
//!
//! ```rust,no_run
//! # use soroban_testkit_core::StackDepthAuditor;
//! # let env = soroban_sdk::Env::default();
//! let mut auditor = StackDepthAuditor::new();
//! auditor.observe(&env); // after the top-level call returns
//! auditor.assert_within();
//! ```
//!
//! # What the diagnostic events can and cannot show
//!
//! The reconstruction is exact for successful calls: the host emits one
//! `fn_call` before opening a frame and one `fn_return` when the call returns.
//! A call that traps emits no `fn_return`, so its frame stays open and the
//! trace records its depth but cannot pair it with a later return. The
//! high-water depth of the failing chain is still correct. Events are only
//! recorded while the test environment has diagnostics enabled, which
//! `Env::default()` does.

use soroban_env_host::events::{Events, HostEvent};
use soroban_env_host::xdr::{ContractEventBody, ContractEventType, ScVal};
use soroban_sdk::Env;
use std::fmt;

/// The host's context-stack ceiling, in frames.
///
/// This is [`soroban_env_host::DEFAULT_HOST_DEPTH_LIMIT`] rather than a local
/// guess: `soroban-env-host` 28.0.2 sets it to `100` in
/// `src/budget/limits.rs`, and `Host::with_frame` rejects a new frame once the
/// context stack has reached it.
///
/// A test environment's top-level client call starts with an empty context
/// stack, so 100 contract frames are accepted and the 101st fails. An entry
/// point reached through `Host::invoke_function` pushes a host-function frame
/// first and therefore has room for 99 contract frames.
pub const HOST_CALL_STACK_LIMIT: u32 = soroban_env_host::DEFAULT_HOST_DEPTH_LIMIT;

/// The default warning threshold: 90% of [`HOST_CALL_STACK_LIMIT`].
pub const DEFAULT_WARNING_THRESHOLD: u32 = HOST_CALL_STACK_LIMIT / 10 * 9;

/// One contract invocation in a call chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallFrame {
    /// Nesting depth, starting at `1` for the top-level call.
    pub depth: u32,
    /// The called contract id, as its `C...` strkey.
    pub contract: String,
    /// The function that was invoked.
    pub function: String,
}

impl fmt::Display for CallFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "depth={} contract={} function={}",
            self.depth, self.contract, self.function
        )
    }
}

/// A call chain that went deeper than the auditor's cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackDepthBreach {
    /// The deepest frame observed.
    pub max_depth: u32,
    /// The cap the trace was checked against.
    pub cap: u32,
    /// The full rendered call trace.
    pub trace: String,
}

impl fmt::Display for StackDepthBreach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "call-stack depth {} exceeded the configured cap {}\n{}",
            self.max_depth, self.cap, self.trace
        )
    }
}

impl std::error::Error for StackDepthBreach {}

/// Tracks the depth of contract calls and checks it against a configurable cap.
///
/// An auditor can be fed frames by hand with [`StackDepthAuditor::enter`] and
/// [`StackDepthAuditor::exit`], which is what an instrumented harness or test
/// contract uses, or it can read the frames the host already recorded with
/// [`StackDepthAuditor::observe`].
#[derive(Debug, Clone)]
pub struct StackDepthAuditor {
    cap: u32,
    warning_threshold: u32,
    frames: Vec<CallFrame>,
    open: Vec<(String, String, u32)>,
    max_depth: u32,
    consumed_events: usize,
}

impl Default for StackDepthAuditor {
    fn default() -> Self {
        Self::new()
    }
}

impl StackDepthAuditor {
    /// Create an auditor capped at the host's own limit.
    pub fn new() -> Self {
        Self {
            cap: HOST_CALL_STACK_LIMIT,
            warning_threshold: DEFAULT_WARNING_THRESHOLD,
            frames: Vec::new(),
            open: Vec::new(),
            max_depth: 0,
            consumed_events: 0,
        }
    }

    /// Create an auditor and read the diagnostic events already recorded in
    /// `env`.
    pub fn from_env(env: &Env) -> Self {
        let mut auditor = Self::new();
        auditor.observe(env);
        auditor
    }

    /// Replace the cap the trace is checked against.
    ///
    /// The cap is a test-side emulation of the host limit, not a change to the
    /// host: a breach is reported by [`StackDepthAuditor::breach`] and
    /// [`StackDepthAuditor::assert_within`], never by the runtime.
    pub fn with_cap(mut self, cap: u32) -> Self {
        self.cap = cap;
        self.warning_threshold = self.warning_threshold.min(cap);
        self
    }

    /// Set the depth at which [`StackDepthAuditor::warning_triggered`] becomes
    /// true. Values above the cap are clamped to it.
    pub fn with_warning_threshold(mut self, threshold: u32) -> Self {
        self.warning_threshold = threshold.min(self.cap);
        self
    }

    /// Lower the cap, for tests that exercise a contract's recovery path
    /// without running the full host depth.
    ///
    /// # Panics
    ///
    /// Panics when `cap` is above the auditor's current cap, since this method
    /// only lowers it.
    pub fn with_lowered_cap(self, cap: u32) -> Self {
        assert!(
            cap <= self.cap,
            "with_lowered_cap: {cap} is above the current cap {}",
            self.cap
        );
        self.with_cap(cap)
    }

    /// The cap currently in force.
    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// The depth at which a warning is raised.
    pub fn warning_threshold(&self) -> u32 {
        self.warning_threshold
    }

    /// Every frame recorded so far, in call order.
    pub fn frames(&self) -> &[CallFrame] {
        &self.frames
    }

    /// The deepest frame observed.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// Record the entry of a contract function into the call chain.
    ///
    /// Returns the depth assigned to the frame, starting at `1`.
    pub fn enter(&mut self, contract: impl Into<String>, function: impl Into<String>) -> &mut Self {
        let contract = contract.into();
        let function = function.into();
        let depth = self.open.len() as u32 + 1;
        self.frames.push(CallFrame {
            depth,
            contract: contract.clone(),
            function: function.clone(),
        });
        self.open.push((contract, function, depth));
        self.max_depth = self.max_depth.max(depth);
        self
    }

    /// Record the return from the most recently entered frame.
    ///
    /// A return without a matching entry is ignored, so a harness can call
    /// this unconditionally.
    pub fn exit(&mut self) -> &mut Self {
        self.open.pop();
        self
    }

    /// Read the diagnostic events recorded in `env` since the last call and
    /// turn them into frames.
    ///
    /// The host records `fn_call` for a successful or failing invocation and
    /// `fn_return` when it returns. Cross-contract calls appear in the same
    /// stream, which is what makes the reconstruction cover a whole chain.
    /// Calling this repeatedly consumes only the new events.
    pub fn observe(&mut self, env: &Env) -> &mut Self {
        let events = self.diagnostic_events(env);
        for event in events.0.iter().skip(self.consumed_events) {
            self.observe_event(event);
        }
        self.consumed_events = events.0.len();
        self
    }

    /// Discard everything recorded so far.
    ///
    /// The diagnostic-event cursor is reset too, so the next
    /// [`StackDepthAuditor::observe`] re-reads the whole event log of `env`.
    pub fn reset(&mut self) -> &mut Self {
        self.frames.clear();
        self.open.clear();
        self.max_depth = 0;
        self.consumed_events = 0;
        self
    }

    /// Whether any frame reached the warning threshold.
    pub fn warning_triggered(&self) -> bool {
        self.max_depth >= self.warning_threshold && self.max_depth > 0
    }

    /// Frames at or beyond the warning threshold, deepest last.
    pub fn warnings(&self) -> Vec<&CallFrame> {
        self.frames
            .iter()
            .filter(|frame| frame.depth >= self.warning_threshold)
            .collect()
    }

    /// The breach, if the deepest frame went past the cap.
    pub fn breach(&self) -> Option<StackDepthBreach> {
        self.breach_against(self.cap)
    }

    /// Check the recorded trace against a lower cap, as if the host enforced
    /// it.
    ///
    /// This does not mutate the auditor: it is the simulation hook for tests
    /// that need a shallower ceiling than the real host allows. Pass the
    /// deepest depth the contract allows before its recovery path and observe
    /// that it is selected; the caller decides what a breach means.
    pub fn simulate_lowered_cap(&self, lowered_cap: u32) -> Option<StackDepthBreach> {
        self.breach_against(lowered_cap)
    }

    /// Panic with the full call trace when the cap was exceeded.
    ///
    /// # Panics
    ///
    /// Panics when [`StackDepthAuditor::breach`] returns `Some`.
    pub fn assert_within(&self) {
        if let Some(breach) = self.breach() {
            panic!("{breach}");
        }
    }

    /// Render every frame with its depth, contract id and function.
    pub fn trace(&self) -> String {
        let mut trace = format!(
            "call trace: {} frame(s), max depth {}, cap {}, warning threshold {}",
            self.frames.len(),
            self.max_depth,
            self.cap,
            self.warning_threshold
        );
        for frame in &self.frames {
            trace.push_str("\n  ");
            trace.push_str(&frame.to_string());
        }
        trace
    }

    fn breach_against(&self, cap: u32) -> Option<StackDepthBreach> {
        (self.max_depth > cap).then(|| StackDepthBreach {
            max_depth: self.max_depth,
            cap,
            trace: self.trace(),
        })
    }

    fn diagnostic_events(&self, env: &Env) -> Events {
        env.host()
            .get_diagnostic_events()
            .expect("the test host exposes its recorded diagnostic events")
    }

    fn observe_event(&mut self, event: &HostEvent) {
        let ContractEventType::Diagnostic = event.event.type_ else {
            return;
        };
        let ContractEventBody::V0(body) = &event.event.body;
        let Some(ScVal::Symbol(topic)) = body.topics.first() else {
            return;
        };
        match topic.0.as_slice() {
            b"fn_call" => {
                let (Some(contract), Some(function)) =
                    (contract_id(&body.topics, 1), symbol(&body.topics, 2))
                else {
                    return;
                };
                self.enter(contract, function);
            }
            b"fn_return" => {
                self.exit();
            }
            _ => {}
        }
    }
}

/// The called contract id of an `fn_call` diagnostic event, as a `C...`
/// strkey.
fn contract_id(topics: &[ScVal], index: usize) -> Option<String> {
    let ScVal::Bytes(bytes) = topics.get(index)? else {
        return None;
    };
    let bytes: [u8; 32] = bytes.0.as_slice().try_into().ok()?;
    Some(contract_strkey(&bytes))
}

/// A symbol-valued topic, as a function name.
fn symbol(topics: &[ScVal], index: usize) -> Option<String> {
    let ScVal::Symbol(symbol) = topics.get(index)? else {
        return None;
    };
    Some(String::from_utf8_lossy(symbol.0.as_slice()).into_owned())
}

/// Encode a contract id as its `C...` strkey: version byte `0x10`, the 32-byte
/// payload, a CRC16-XModem checksum, and RFC 4648 base32 without padding.
fn contract_strkey(bytes: &[u8; 32]) -> String {
    const VERSION: u8 = 2 << 3;
    let mut raw = Vec::with_capacity(35);
    raw.push(VERSION);
    raw.extend_from_slice(bytes);
    let checksum = crc16_xmodem(&raw);
    raw.extend_from_slice(&checksum.to_le_bytes());
    base32(&raw)
}

fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn base32(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut output = String::with_capacity(data.len().div_ceil(5) * 8);
    for chunk in data.chunks(5) {
        let mut buffer = [0u8; 5];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from(buffer[0]) << 32
            | u64::from(buffer[1]) << 24
            | u64::from(buffer[2]) << 16
            | u64::from(buffer[3]) << 8
            | u64::from(buffer[4]);
        let characters = chunk.len() * 8 / 5 + usize::from(chunk.len() * 8 % 5 != 0);
        for index in 0..characters {
            let shift = 35 - index * 5;
            output.push(ALPHABET[((bits >> shift) & 0x1f) as usize] as char);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        contract_strkey, CallFrame, StackDepthAuditor, DEFAULT_WARNING_THRESHOLD,
        HOST_CALL_STACK_LIMIT,
    };
    use soroban_sdk::xdr::{ContractId, Hash, ScAddress};
    use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, TryFromVal as _};

    /// A handful of frames recorded by hand, which is what an instrumented
    /// harness feeds the auditor.
    fn delegation_chain() -> StackDepthAuditor {
        let mut auditor = StackDepthAuditor::new();
        auditor
            .enter("CPROXY", "forward")
            .enter("CPROXY2", "relay")
            .enter("CIMPL", "credit");
        auditor
    }

    #[test]
    fn a_recursive_loop_records_every_frame() {
        let mut auditor = StackDepthAuditor::new();
        for _ in 0..25 {
            auditor.enter("CLOOP", "recurse");
        }

        assert_eq!(auditor.max_depth(), 25);
        assert_eq!(auditor.frames().len(), 25);
        assert_eq!(auditor.frames()[24].depth, 25);
        assert_eq!(auditor.frames()[24].function, "recurse");
    }

    #[test]
    fn a_deep_delegation_chain_keeps_insertion_order() {
        let auditor = delegation_chain();

        assert_eq!(auditor.max_depth(), 3);
        assert_eq!(
            auditor
                .frames()
                .iter()
                .map(|frame| (
                    frame.depth,
                    frame.contract.as_str(),
                    frame.function.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                (1, "CPROXY", "forward"),
                (2, "CPROXY2", "relay"),
                (3, "CIMPL", "credit"),
            ]
        );
    }

    #[test]
    fn exiting_a_frame_lets_a_sibling_reuse_its_depth() {
        let mut auditor = StackDepthAuditor::new();
        auditor.enter("CROOT", "run").enter("CA", "left").exit();
        auditor.enter("CB", "right");

        assert_eq!(auditor.max_depth(), 2);
        assert_eq!(auditor.frames().last().unwrap().depth, 2);
        assert_eq!(auditor.frames().last().unwrap().contract, "CB");
    }

    #[test]
    fn an_unmatched_exit_is_ignored() {
        let mut auditor = StackDepthAuditor::new();
        auditor.exit();
        auditor.enter("CONLY", "one");

        assert_eq!(auditor.max_depth(), 1);
    }

    #[test]
    fn the_default_cap_is_the_host_limit() {
        let auditor = StackDepthAuditor::new();
        assert_eq!(auditor.cap(), HOST_CALL_STACK_LIMIT);
        assert_eq!(auditor.warning_threshold(), DEFAULT_WARNING_THRESHOLD);
        assert_eq!(HOST_CALL_STACK_LIMIT, 100);
    }

    #[test]
    fn the_warning_threshold_fires_before_the_cap() {
        let auditor = StackDepthAuditor::new()
            .with_cap(10)
            .with_warning_threshold(8);

        let mut shallow = auditor.clone();
        for _ in 0..7 {
            shallow.enter("C", "f");
        }
        assert!(!shallow.warning_triggered());
        assert!(shallow.warnings().is_empty());
        assert!(shallow.breach().is_none());

        let mut near = auditor.clone();
        for _ in 0..8 {
            near.enter("C", "f");
        }
        assert!(near.warning_triggered());
        assert_eq!(near.warnings().len(), 1);
        assert!(near.breach().is_none(), "a warning is not a breach");
    }

    #[test]
    fn the_cap_is_enforced_and_the_breach_carries_the_trace() {
        let auditor = delegation_chain();
        let breach = auditor
            .with_cap(2)
            .breach()
            .expect("three frames exceed a cap of two");

        assert_eq!(breach.max_depth, 3);
        assert_eq!(breach.cap, 2);
        assert!(breach.trace.contains("function=forward"));
        assert!(breach.trace.contains("function=credit"));
        assert!(breach.to_string().contains("exceeded the configured cap 2"));
    }

    #[test]
    #[should_panic(expected = "call-stack depth 3 exceeded the configured cap 2")]
    fn assert_within_panics_with_the_trace() {
        delegation_chain().with_cap(2).assert_within();
    }

    #[test]
    fn a_trace_within_the_cap_does_not_panic() {
        delegation_chain().with_cap(3).assert_within();
    }

    #[test]
    fn a_lowered_cap_simulates_the_host_limit() {
        let auditor = delegation_chain();

        // The real host would accept this chain; the simulation reports what a
        // three-frame contract would see behind a two-frame ceiling.
        assert!(auditor.breach().is_none());
        let breach = auditor
            .simulate_lowered_cap(2)
            .expect("a two-frame cap rejects the third frame");
        assert_eq!(breach.cap, 2);
        assert_eq!(auditor.max_depth(), 3, "the simulation does not mutate");
    }

    #[test]
    fn a_lowered_cap_builder_cannot_raise_the_cap() {
        let auditor = StackDepthAuditor::new().with_cap(10).with_lowered_cap(4);
        assert_eq!(auditor.cap(), 4);
        assert_eq!(auditor.warning_threshold(), 4);
    }

    #[test]
    #[should_panic(expected = "is above the current cap")]
    fn a_lowered_cap_builder_refuses_to_raise_the_cap() {
        let _ = StackDepthAuditor::new().with_cap(4).with_lowered_cap(5);
    }

    #[test]
    fn the_trace_lists_every_frame_with_depth_contract_and_function() {
        let auditor = delegation_chain();
        let trace = auditor.trace();

        assert!(trace.contains("3 frame(s), max depth 3"), "{trace}");
        assert!(trace.contains("depth=1 contract=CPROXY function=forward"));
        assert!(trace.contains("depth=2 contract=CPROXY2 function=relay"));
        assert!(trace.contains("depth=3 contract=CIMPL function=credit"));
    }

    #[test]
    fn the_frame_display_is_key_value_pairs() {
        let frame = CallFrame {
            depth: 7,
            contract: "CABC".to_string(),
            function: "ping".to_string(),
        };
        assert_eq!(frame.to_string(), "depth=7 contract=CABC function=ping");
    }

    #[test]
    fn a_wide_but_shallow_tree_has_no_false_positive() {
        let mut auditor = StackDepthAuditor::new()
            .with_cap(5)
            .with_warning_threshold(4);
        auditor.enter("CROOT", "fan_out");
        for _ in 0..64 {
            auditor.enter("CLEAF", "ping").exit();
        }
        auditor.exit();

        assert_eq!(auditor.max_depth(), 2);
        assert!(!auditor.warning_triggered());
        assert!(auditor.warnings().is_empty());
        assert!(auditor.breach().is_none());
        auditor.assert_within();
    }

    #[test]
    fn reset_discards_the_frames() {
        let mut auditor = delegation_chain();
        auditor.reset();

        assert_eq!(auditor.max_depth(), 0);
        assert!(auditor.frames().is_empty());
        assert!(auditor.breach().is_none());
    }

    #[test]
    fn contract_strkeys_match_the_sdk_encoding() {
        let env = Env::default();
        let expected =
            Address::try_from_val(&env, &ScAddress::Contract(ContractId(Hash([0u8; 32]))))
                .expect("a zero contract id is a valid address");
        assert_eq!(
            contract_strkey(&[0u8; 32]),
            format!("{}", expected.to_string()),
            "the encoder must agree with the SDK's strkey output"
        );
    }

    // --- Real host call chains -------------------------------------------

    #[contract]
    pub struct Leaf;

    #[contractimpl]
    impl Leaf {
        pub fn ping(_env: Env, value: u32) -> u32 {
            value + 1
        }

        /// Calls itself through the host, which the host allows as an
        /// immediate self re-entry.
        pub fn recurse(env: Env, remaining: u32) -> u32 {
            if remaining == 0 {
                0
            } else {
                LeafClient::new(&env, &env.current_contract_address()).recurse(&(remaining - 1)) + 1
            }
        }
    }

    #[contract]
    pub struct Middle;

    #[contractimpl]
    impl Middle {
        pub fn configure(env: Env, leaf: Address) {
            env.storage().instance().set(&symbol_short!("leaf"), &leaf);
        }

        pub fn relay(env: Env, value: u32) -> u32 {
            let leaf: Address = env
                .storage()
                .instance()
                .get(&symbol_short!("leaf"))
                .expect("configure was called");
            LeafClient::new(&env, &leaf).ping(&value)
        }
    }

    #[contract]
    pub struct Proxy;

    #[contractimpl]
    impl Proxy {
        pub fn configure(env: Env, middle: Address) {
            env.storage()
                .instance()
                .set(&symbol_short!("middle"), &middle);
        }

        pub fn forward(env: Env, value: u32) -> u32 {
            let middle: Address = env
                .storage()
                .instance()
                .get(&symbol_short!("middle"))
                .expect("configure was called");
            MiddleClient::new(&env, &middle).relay(&value)
        }
    }

    fn proxy_chain() -> (Env, Address, Address, Address) {
        let env = Env::default();
        let leaf = env.register(Leaf, ());
        let middle = env.register(Middle, ());
        let proxy = env.register(Proxy, ());
        MiddleClient::new(&env, &middle).configure(&leaf);
        ProxyClient::new(&env, &proxy).configure(&middle);
        (env, proxy, middle, leaf)
    }

    #[test]
    fn observing_the_host_reconstructs_a_cross_contract_chain() {
        let (env, proxy, middle, leaf) = proxy_chain();
        ProxyClient::new(&env, &proxy).forward(&1);

        let auditor = StackDepthAuditor::from_env(&env);
        let proxy_id = format!("{}", proxy.to_string());
        let middle_id = format!("{}", middle.to_string());
        let leaf_id = format!("{}", leaf.to_string());
        assert_eq!(auditor.max_depth(), 3, "{}", auditor.trace());
        assert_eq!(
            auditor
                .frames()
                .iter()
                .map(|frame| (
                    frame.depth,
                    frame.contract.as_str(),
                    frame.function.as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                (1, proxy_id.as_str(), "forward"),
                (2, middle_id.as_str(), "relay"),
                (3, leaf_id.as_str(), "ping"),
            ]
        );
        assert!(auditor.trace().contains(&format!("contract={leaf_id}")));
    }

    #[test]
    fn observing_a_refused_reentry_does_not_invent_a_frame() {
        let env = Env::default();
        let leaf = env.register(Leaf, ());
        // The host refuses contract re-entry with `Context/InvalidAction`
        // before it emits the nested `fn_call`, so the refused call never
        // becomes a frame. The depth of the call that did run is still exact.
        let result = LeafClient::new(&env, &leaf).try_recurse(&3);
        assert!(result.is_err(), "the host must refuse re-entry");

        let auditor = StackDepthAuditor::from_env(&env);
        assert_eq!(auditor.max_depth(), 1, "{}", auditor.trace());
        assert_eq!(auditor.frames().len(), 1);
        assert_eq!(auditor.frames()[0].function, "recurse");
    }

    #[test]
    fn observing_a_wide_but_shallow_host_chain_does_not_warn() {
        let env = Env::default();
        let leaf = env.register(Leaf, ());
        let client = LeafClient::new(&env, &leaf);
        for value in 0..16 {
            client.ping(&value);
        }

        let auditor = StackDepthAuditor::from_env(&env)
            .with_cap(4)
            .with_warning_threshold(3);
        assert_eq!(auditor.max_depth(), 1);
        assert!(!auditor.warning_triggered());
        assert!(auditor.breach().is_none());
    }

    #[test]
    fn repeated_observation_does_not_double_count() {
        let (env, proxy, _, _) = proxy_chain();
        let client = ProxyClient::new(&env, &proxy);
        client.forward(&1);

        let mut auditor = StackDepthAuditor::new();
        auditor.observe(&env);
        let first = auditor.frames().len();
        auditor.observe(&env);

        assert_eq!(auditor.frames().len(), first);
        assert_eq!(auditor.max_depth(), 3);
    }
}
