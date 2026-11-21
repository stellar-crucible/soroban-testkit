//! Asserting the diagnostic message a panicking invocation produced.
//!
//! `#[should_panic(expected = "...")]` only does a substring match, applies to
//! the whole test, and passes for a panic raised anywhere in it.
//! [`assert_panic_msg!`] scopes the expectation to one expression and matches
//! its panic message by substring, glob or regular expression:
//!
//! ```rust
//! use soroban_testkit_assert::assert_panic_msg;
//!
//! fn withdraw(amount: u64) {
//!     assert!(amount <= 100, "withdrawal of {amount} exceeds the limit of 100");
//! }
//!
//! assert_panic_msg!(withdraw(250), "exceeds the limit");
//! assert_panic_msg!(withdraw(250), contains: "exceeds the limit");
//! assert_panic_msg!(withdraw(250), glob: "withdrawal of * exceeds the limit of ???");
//! assert_panic_msg!(withdraw(250), regex: r"^withdrawal of \d+ exceeds");
//!
//! let amount = 250;
//! let captured = assert_panic_msg!(withdraw(amount), contains: format!("withdrawal of {amount}"));
//! assert_eq!(captured.message(), "withdrawal of 250 exceeds the limit of 100");
//! ```
//!
//! # Where the message comes from
//!
//! A panic payload is an arbitrary `Box<dyn Any + Send>`. `panic!` with a
//! format string produces a `String`, `panic!("literal")` and a bare
//! `assert!(cond)` produce a `&'static str`, and `std::panic::panic_any` can
//! carry anything. Only `String` and `&str` payloads have a message; any other
//! payload is reported as [`PanicMsgFailure::NonTextPayload`] rather than being
//! matched as an empty string.
//!
//! A contract invoked through a generated client in a test `Env` does not
//! unwind with the contract's own payload. The host catches the contract's
//! panic, turns it into a host error and re-panics with a `String` of the
//! form:
//!
//! ```text
//! HostError: Error(WasmVm, InvalidAction)
//!
//! Event log (newest first):
//!    ...
//!    3: [Failed Diagnostic Event (not emitted)] ..., data:["caught panic 'amount 5 too large' from contract function 'Symbol(bid)'", 5]
//! ```
//!
//! [`CapturedPanic`] parses that text: [`CapturedPanic::host_error`] holds the
//! `Error(..)` rendering and [`CapturedPanic::contract_panics`] the messages
//! from every `caught panic '..'` entry. A pattern passes when it matches
//! **any** of the candidates [`CapturedPanic::candidates`] lists: each
//! contract panic message, the host error, the `HostError: ..` header line
//! and the full raw text. That is what lets an anchored glob or regex such as
//! `glob: "amount * too large"` match a contract's panic even though the raw
//! payload also carries the host's event log.
//!
//! The contract-side message only exists because the test `Env` records
//! diagnostic events (`Env::default()` turns them on). With diagnostics off,
//! or for an error raised with `panic_with_error!` (which carries a code, not
//! text), only the host error is available; the failure diagnostic says so
//! instead of pretending the message was empty.

use std::any::Any;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// How a [`PanicPattern`] compares itself with a panic message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// The message contains the pattern verbatim.
    Contains,
    /// The whole message matches a glob: `*` is any run of characters
    /// (including none and including newlines), `?` exactly one character,
    /// and every other character matches itself.
    Glob,
    /// The regular expression finds a match somewhere in the message (anchor
    /// with `^` and `$` to require a whole-message match).
    Regex,
}

impl MatchMode {
    /// The keyword the macro uses to select this mode.
    pub fn keyword(self) -> &'static str {
        match self {
            MatchMode::Contains => "contains",
            MatchMode::Glob => "glob",
            MatchMode::Regex => "regex",
        }
    }
}

impl fmt::Display for MatchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.keyword())
    }
}

/// An expected panic message: the mode and the pattern text.
#[derive(Debug, Clone)]
pub struct PanicPattern {
    mode: MatchMode,
    source: String,
    regex: Option<regex::Regex>,
}

impl PanicPattern {
    /// Matches messages containing `text` verbatim.
    pub fn contains(text: impl Into<String>) -> Self {
        Self {
            mode: MatchMode::Contains,
            source: text.into(),
            regex: None,
        }
    }

    /// Matches messages the glob `pattern` covers entirely.
    pub fn glob(pattern: impl Into<String>) -> Self {
        Self {
            mode: MatchMode::Glob,
            source: pattern.into(),
            regex: None,
        }
    }

    /// Matches messages in which the regular expression finds a match.
    ///
    /// Returns [`PanicMsgFailure::InvalidPattern`] when `pattern` does not
    /// compile, so the macro can refuse the assertion before running the
    /// code under test.
    pub fn regex(pattern: impl Into<String>) -> Result<Self, PanicMsgFailure> {
        let source = pattern.into();
        match regex::Regex::new(&source) {
            Ok(compiled) => Ok(Self {
                mode: MatchMode::Regex,
                source,
                regex: Some(compiled),
            }),
            Err(error) => Err(PanicMsgFailure::InvalidPattern {
                mode: MatchMode::Regex,
                pattern: source,
                error: error.to_string(),
            }),
        }
    }

    /// The matching mode.
    pub fn mode(&self) -> MatchMode {
        self.mode
    }

    /// The pattern text as written.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether this pattern matches `text` alone, with no candidate
    /// expansion.
    pub fn matches_text(&self, text: &str) -> bool {
        match self.mode {
            MatchMode::Contains => text.contains(self.source.as_str()),
            MatchMode::Glob => glob_match(&self.source, text),
            MatchMode::Regex => self
                .regex
                .as_ref()
                .is_some_and(|compiled| compiled.is_match(text)),
        }
    }

    /// The first of `captured`'s candidates this pattern matches, if any.
    pub fn find_match<'a>(&self, captured: &'a CapturedPanic) -> Option<&'a str> {
        captured
            .candidates()
            .into_iter()
            .find(|candidate| self.matches_text(candidate))
    }
}

impl fmt::Display for PanicPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?}", self.mode, self.source)
    }
}

/// Whole-string glob matching over `char`s: `*` matches any run of
/// characters, `?` exactly one character.
///
/// Iterative with single-star backtracking, so it runs in
/// `O(pattern × text)` time and never recurses.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0usize, 0usize);
    let mut backtrack: Option<(usize, usize)> = None;

    while t < txt.len() {
        if p < pat.len() && pat[p] == '*' {
            backtrack = Some((p, t));
            p += 1;
        } else if p < pat.len() && (pat[p] == '?' || pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = backtrack {
            p = star_p + 1;
            t = star_t + 1;
            backtrack = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pat[p..].iter().all(|&c| c == '*')
}

/// Where a captured panic came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanicOrigin {
    /// An ordinary Rust panic (`panic!`, `assert!`, `unwrap`, ...) raised by
    /// test-side code.
    Rust,
    /// A Soroban host error escalated to a panic, recognised by its
    /// `HostError: ` prefix.
    Host,
}

/// The text of a caught panic, with the Soroban host parts parsed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedPanic {
    /// The full payload text, unmodified.
    pub raw: String,
    /// Whether the payload was a Rust panic or a Soroban host error.
    pub origin: PanicOrigin,
    /// For a host panic, the `Error(..)` rendering from its first line, e.g.
    /// `Error(Contract, #7)` or `Error(WasmVm, InvalidAction)`.
    pub host_error: Option<String>,
    /// For a host panic, the message of every `caught panic '..'` diagnostic
    /// entry, newest first and without duplicates. Empty when the contract
    /// raised an error code instead of panicking, or when diagnostic events
    /// were not recorded.
    pub contract_panics: Vec<String>,
}

const HOST_PREFIX: &str = "HostError: ";
const CAUGHT_PANIC: &str = "caught panic '";
const FROM_CONTRACT: &str = "' from contract function";

impl CapturedPanic {
    /// Parses a panic message's text.
    pub fn parse(text: impl Into<String>) -> Self {
        let raw = text.into();
        if !raw.starts_with(HOST_PREFIX) {
            return Self {
                raw,
                origin: PanicOrigin::Rust,
                host_error: None,
                contract_panics: Vec::new(),
            };
        }

        let header = raw.lines().next().unwrap_or_default();
        let host_error = header
            .strip_prefix(HOST_PREFIX)
            .map(str::trim)
            .filter(|rest| !rest.is_empty())
            .map(str::to_string);

        let mut contract_panics: Vec<String> = Vec::new();
        let mut rest = raw.as_str();
        while let Some(start) = rest.find(CAUGHT_PANIC) {
            let after = &rest[start + CAUGHT_PANIC.len()..];
            let Some(end) = after.find(FROM_CONTRACT) else {
                break;
            };
            let message = after[..end].to_string();
            if !contract_panics.contains(&message) {
                contract_panics.push(message);
            }
            rest = &after[end + FROM_CONTRACT.len()..];
        }

        Self {
            raw,
            origin: PanicOrigin::Host,
            host_error,
            contract_panics,
        }
    }

    /// Reads a panic payload, returning `None` when it is neither a `String`
    /// nor a `&'static str`.
    pub fn from_payload(payload: &(dyn Any + Send)) -> Option<Self> {
        payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .map(Self::parse)
    }

    /// The texts a pattern is tried against, most specific first: each
    /// contract panic message, the host error, the `HostError: ..` header
    /// line, then the full raw text. For a Rust panic this is just the raw
    /// text.
    pub fn candidates(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        out.extend(self.contract_panics.iter().map(String::as_str));
        if let Some(error) = &self.host_error {
            out.push(error);
        }
        if self.origin == PanicOrigin::Host {
            if let Some(header) = self.raw.lines().next() {
                out.push(header);
            }
        }
        out.push(&self.raw);
        out.dedup();
        out
    }

    /// The single most useful message: the newest contract panic message for
    /// a host panic that carried one, the host error for one that did not,
    /// and the raw text for a Rust panic.
    pub fn message(&self) -> &str {
        self.contract_panics
            .first()
            .map(String::as_str)
            .or(self.host_error.as_deref())
            .unwrap_or(&self.raw)
    }
}

/// Why an [`assert_panic_msg!`] assertion failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanicMsgFailure {
    /// The invocation returned normally.
    UnexpectedSuccess { mode: MatchMode, pattern: String },
    /// The invocation panicked with a payload that is neither `String` nor
    /// `&str`, so no message is available to match.
    NonTextPayload { mode: MatchMode, pattern: String },
    /// The invocation panicked, but no candidate message matched.
    Mismatch {
        mode: MatchMode,
        pattern: String,
        captured: CapturedPanic,
    },
    /// The pattern itself could not be compiled; the invocation was not run.
    InvalidPattern {
        mode: MatchMode,
        pattern: String,
        error: String,
    },
}

fn indent(text: &str) -> String {
    text.trim_end()
        .lines()
        .map(|line| format!("    | {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

impl fmt::Display for PanicMsgFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PanicMsgFailure::UnexpectedSuccess { mode, pattern } => write!(
                f,
                "assert_panic_msg! failed: the invocation completed without panicking\n  expected a panic matching ({mode}): {pattern:?}"
            ),
            PanicMsgFailure::NonTextPayload { mode, pattern } => write!(
                f,
                "assert_panic_msg! failed: the invocation panicked, but its payload is neither a String nor a &str, so no message is available to match\n  expected ({mode}): {pattern:?}\n  hint: the panic was raised with a non-text payload, e.g. std::panic::panic_any(value)"
            ),
            PanicMsgFailure::InvalidPattern {
                mode,
                pattern,
                error,
            } => write!(
                f,
                "assert_panic_msg! failed: the {mode} pattern is invalid, so the invocation was not run\n  pattern: {pattern:?}\n  error:\n{}",
                indent(error)
            ),
            PanicMsgFailure::Mismatch {
                mode,
                pattern,
                captured,
            } => {
                write!(
                    f,
                    "assert_panic_msg! failed: the panic message did not match\n  expected ({mode}): {pattern:?}"
                )?;
                match captured.origin {
                    PanicOrigin::Rust => {
                        write!(f, "\n  origin: Rust panic")?;
                        write!(f, "\n  actual:\n{}", indent(&captured.raw))
                    }
                    PanicOrigin::Host => {
                        write!(f, "\n  origin: Soroban host error")?;
                        if let Some(error) = &captured.host_error {
                            write!(f, "\n  host error: {error}")?;
                        }
                        if captured.contract_panics.is_empty() {
                            write!(
                                f,
                                "\n  contract panic message: unavailable — the host's diagnostic event log carried no `caught panic` entry (the contract raised an error code, or diagnostic events were not recorded)"
                            )?;
                        } else {
                            for message in &captured.contract_panics {
                                write!(f, "\n  contract panic message: {message:?}")?;
                            }
                        }
                        write!(f, "\n  actual (full host panic):\n{}", indent(&captured.raw))
                    }
                }
            }
        }
    }
}

impl std::error::Error for PanicMsgFailure {}

/// Runs `f` once inside `catch_unwind`, returning its panic payload if it
/// panicked.
///
/// `f` is wrapped in `AssertUnwindSafe`: the assertion reports the panic, it
/// does not try to keep running with state the closure may have left
/// half-updated.
pub fn capture_panic<F: FnOnce()>(f: F) -> Result<(), Box<dyn Any + Send>> {
    catch_unwind(AssertUnwindSafe(f))
}

/// Checks the outcome of [`capture_panic`] against `pattern`, returning the
/// captured message on a match.
///
/// This is the function [`assert_panic_msg!`] calls; use it directly to
/// inspect a failure without panicking.
pub fn check_panic_msg(
    outcome: Result<(), Box<dyn Any + Send>>,
    pattern: &PanicPattern,
) -> Result<CapturedPanic, PanicMsgFailure> {
    let mode = pattern.mode();
    let source = pattern.as_str().to_string();
    let payload = match outcome {
        Ok(()) => {
            return Err(PanicMsgFailure::UnexpectedSuccess {
                mode,
                pattern: source,
            })
        }
        Err(payload) => payload,
    };
    let Some(captured) = CapturedPanic::from_payload(payload.as_ref()) else {
        return Err(PanicMsgFailure::NonTextPayload {
            mode,
            pattern: source,
        });
    };
    if pattern.find_match(&captured).is_some() {
        Ok(captured)
    } else {
        Err(PanicMsgFailure::Mismatch {
            mode,
            pattern: source,
            captured,
        })
    }
}

/// Runs `f` exactly once and checks its panic message against `pattern`,
/// panicking with the [`PanicMsgFailure`] diagnostic when it does not match.
#[track_caller]
pub fn assert_panic_msg_with<F: FnOnce()>(pattern: &PanicPattern, f: F) -> CapturedPanic {
    match check_panic_msg(capture_panic(f), pattern) {
        Ok(captured) => captured,
        Err(failure) => panic!("{failure}"),
    }
}

/// Assert that an expression panics with a message matching a pattern.
///
/// The expression is evaluated exactly once, inside `catch_unwind`. On a
/// match the macro evaluates to the [`CapturedPanic`](crate::panic_msg::CapturedPanic),
/// so further checks can read the host error or contract message.
///
/// Forms (the keyword selects the mode; the macro never guesses it from the
/// pattern's contents):
///
/// * `assert_panic_msg!(expr, "text")` — substring, the same rule as
///   `#[should_panic(expected = "...")]`;
/// * `assert_panic_msg!(expr, contains: "text")` — substring, spelled out;
/// * `assert_panic_msg!(expr, glob: "amount * too ?arge")` — whole-message
///   glob with `*` and `?`;
/// * `assert_panic_msg!(expr, regex: r"amount \d+")` — regular expression
///   (the `regex` crate's syntax), unanchored unless the pattern uses `^`/`$`.
///
/// The pattern may be any expression yielding `&str` or `String`, so a
/// `format!` with dynamic values works. A closure written in place
/// (`|| expr` or `move || expr`) is called once rather than merely
/// constructed.
///
/// Failures say which of four things went wrong: the invocation did not
/// panic, its payload had no text, no candidate message matched (both the
/// expected pattern and the actual message are printed), or a regex pattern
/// did not compile (reported before the invocation is run).
///
/// ```rust
/// use soroban_testkit_assert::assert_panic_msg;
///
/// let limit = 100;
/// let captured = assert_panic_msg!(
///     || panic!("value 250 exceeds {limit}"),
///     contains: format!("exceeds {limit}")
/// );
/// assert_eq!(captured.message(), "value 250 exceeds 100");
/// ```
#[macro_export]
macro_rules! assert_panic_msg {
    (move || $body:expr, $($rest:tt)+) => {
        $crate::assert_panic_msg!($body, $($rest)+)
    };
    (|| $body:expr, $($rest:tt)+) => {
        $crate::assert_panic_msg!($body, $($rest)+)
    };
    ($call:expr, contains: $pattern:expr $(,)?) => {
        $crate::panic_msg::assert_panic_msg_with(
            &$crate::panic_msg::PanicPattern::contains($pattern),
            || {
                // `$call` may itself be a `panic!`, which is the point.
                #[allow(clippy::diverging_sub_expression, unreachable_code)]
                let _ = $call;
            },
        )
    };
    ($call:expr, glob: $pattern:expr $(,)?) => {
        $crate::panic_msg::assert_panic_msg_with(
            &$crate::panic_msg::PanicPattern::glob($pattern),
            || {
                // `$call` may itself be a `panic!`, which is the point.
                #[allow(clippy::diverging_sub_expression, unreachable_code)]
                let _ = $call;
            },
        )
    };
    ($call:expr, regex: $pattern:expr $(,)?) => {
        match $crate::panic_msg::PanicPattern::regex($pattern) {
            Ok(pattern) => $crate::panic_msg::assert_panic_msg_with(&pattern, || {
                // `$call` may itself be a `panic!`, which is the point.
                #[allow(clippy::diverging_sub_expression, unreachable_code)]
                let _ = $call;
            }),
            Err(failure) => panic!("{}", failure),
        }
    };
    ($call:expr, $pattern:expr $(,)?) => {
        $crate::assert_panic_msg!($call, contains: $pattern)
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_and_question_mark() {
        assert!(glob_match("a*c", "abbbc"));
        assert!(glob_match("a*c", "ac"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
        assert!(glob_match("*", ""));
        assert!(glob_match("**x**", "x"));
        assert!(!glob_match("abc", "abcd"));
        assert!(!glob_match("", "a"));
        assert!(glob_match("*.b", "a.b.b"));
    }

    #[test]
    fn glob_question_mark_is_one_character_not_one_byte() {
        assert!(glob_match("caf?", "café"));
        assert!(!glob_match("caf??", "café"));
    }

    #[test]
    fn glob_star_spans_newlines() {
        assert!(glob_match("left*right", "left\nmiddle\nright"));
    }

    #[test]
    fn rust_panic_text_has_a_single_candidate() {
        let captured = CapturedPanic::parse("boom");
        assert_eq!(captured.origin, PanicOrigin::Rust);
        assert_eq!(captured.candidates(), vec!["boom"]);
        assert_eq!(captured.message(), "boom");
    }

    #[test]
    fn host_panic_text_is_parsed_into_parts() {
        let raw = "HostError: Error(WasmVm, InvalidAction)\n\nEvent log (newest first):\n   3: [Failed Diagnostic Event (not emitted)] contract:C, topics:[log], data:[\"caught panic 'it's 5' from contract function 'Symbol(p)'\", 5]\n   4: x";
        let captured = CapturedPanic::parse(raw);
        assert_eq!(captured.origin, PanicOrigin::Host);
        assert_eq!(
            captured.host_error.as_deref(),
            Some("Error(WasmVm, InvalidAction)")
        );
        assert_eq!(captured.contract_panics, vec!["it's 5".to_string()]);
        assert_eq!(captured.message(), "it's 5");
        assert_eq!(
            captured.candidates(),
            vec![
                "it's 5",
                "Error(WasmVm, InvalidAction)",
                "HostError: Error(WasmVm, InvalidAction)",
                raw
            ]
        );
    }

    #[test]
    fn invalid_regex_is_reported_with_its_error() {
        let failure = PanicPattern::regex("amount (").unwrap_err();
        let text = failure.to_string();
        assert!(text.contains("regex pattern is invalid"), "{text}");
        assert!(text.contains("\"amount (\""), "{text}");
    }

    #[test]
    fn non_text_payload_is_not_treated_as_empty() {
        let pattern = PanicPattern::contains("");
        let failure = check_panic_msg(Err(Box::new(42u32)), &pattern).unwrap_err();
        assert!(matches!(failure, PanicMsgFailure::NonTextPayload { .. }));
    }
}
