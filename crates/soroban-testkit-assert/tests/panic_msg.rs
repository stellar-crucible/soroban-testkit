//! Integration tests for `assert_panic_msg!`, imported the way a downstream
//! crate would use it, against both Rust panics and real Soroban host panics.

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};

use soroban_env_host::DiagnosticLevel;
use soroban_sdk::{contract, contracterror, contractimpl, panic_with_error, Env};
use soroban_testkit_assert::panic_msg::{capture_panic, check_panic_msg};
use soroban_testkit_assert::{
    assert_panic_msg, MatchMode, PanicMsgFailure, PanicOrigin, PanicPattern,
};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum VaultError {
    Locked = 7,
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    pub fn withdraw(_env: Env, amount: u32) -> u32 {
        if amount > 100 {
            panic!("withdrawal of {amount} exceeds the limit of 100");
        }
        amount
    }

    pub fn checked(_env: Env, amount: u32) -> u32 {
        assert!(amount == 0, "amount must be zero, got {amount}");
        amount
    }

    pub fn locked(env: Env) -> u32 {
        panic_with_error!(&env, VaultError::Locked)
    }
}

fn vault(env: &Env) -> VaultClient<'_> {
    VaultClient::new(env, &env.register(Vault, ()))
}

/// The message an `assert_panic_msg!` failure panicked with.
fn failure_of(f: impl FnOnce()) -> String {
    let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("the assertion should have failed");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("assertion failures are text")
}

// ---------------------------------------------------------------- matching

#[test]
fn substring_matches_a_rust_panic() {
    assert_panic_msg!(panic!("vault is sealed"), "is sealed");
    assert_panic_msg!(panic!("vault is sealed"), contains: "vault is");
}

#[test]
fn substring_matches_a_dynamic_message() {
    let amount = 250;
    let limit = 100;
    let captured = assert_panic_msg!(
        panic!("withdrawal of {amount} exceeds the limit of {limit}"),
        contains: format!("of {amount} exceeds the limit of {limit}")
    );
    assert_eq!(captured.origin, PanicOrigin::Rust);
    assert_eq!(
        captured.message(),
        "withdrawal of 250 exceeds the limit of 100"
    );
}

#[test]
fn glob_matches_the_whole_message() {
    assert_panic_msg!(
        panic!("withdrawal of 250 exceeds the limit of 100"),
        glob: "withdrawal of * exceeds the limit of ???"
    );
}

#[test]
fn regex_matches_a_message() {
    assert_panic_msg!(
        panic!("withdrawal of 250 exceeds the limit of 100"),
        regex: r"^withdrawal of \d+ exceeds the limit of \d+$"
    );
}

#[test]
fn panic_from_a_failed_assert_with_a_message() {
    let balance = 3;
    assert_panic_msg!(
        assert!(balance > 10, "balance {balance} below minimum"),
        "balance 3 below minimum"
    );
}

#[test]
fn panic_from_a_bare_failed_assert_has_a_static_str_payload() {
    let ready = false;
    assert_panic_msg!(assert!(ready), glob: "assertion failed: ready");
}

#[test]
fn panic_from_a_failed_assert_eq() {
    assert_panic_msg!(assert_eq!(1 + 1, 3), regex: r"(?s)left == right.*left: 2.*right: 3");
}

#[test]
fn closures_written_in_place_are_called() {
    assert_panic_msg!(|| panic!("from a closure"), "from a closure");
    let code = 9;
    assert_panic_msg!(move || panic!("code {code}"), glob: "code ?");
}

// ------------------------------------------------------- host diagnostics

#[test]
fn host_panic_from_contract_panic_is_matched_by_its_own_text() {
    let env = Env::default();
    let client = vault(&env);

    let captured = assert_panic_msg!(
        client.withdraw(&250),
        glob: "withdrawal of * exceeds the limit of 100"
    );
    assert_eq!(captured.origin, PanicOrigin::Host);
    assert_eq!(
        captured.host_error.as_deref(),
        Some("Error(WasmVm, InvalidAction)")
    );
    assert_eq!(
        captured.contract_panics,
        vec!["withdrawal of 250 exceeds the limit of 100".to_string()]
    );

    assert_panic_msg!(client.withdraw(&250), regex: r"^withdrawal of 250 exceeds");
    assert_panic_msg!(client.withdraw(&250), "exceeds the limit");
}

#[test]
fn host_panic_from_contract_assert_is_matched() {
    let env = Env::default();
    let client = vault(&env);
    assert_panic_msg!(client.checked(&5), glob: "amount must be zero, got ?");
}

#[test]
fn host_error_code_is_matched_through_the_host_error_rendering() {
    let env = Env::default();
    let client = vault(&env);
    let captured = assert_panic_msg!(client.locked(), glob: "Error(Contract, #7)");
    assert!(captured.contract_panics.is_empty());
    assert_eq!(captured.message(), "Error(Contract, #7)");
}

#[test]
fn host_mismatch_reports_host_error_contract_message_and_raw_text() {
    let env = Env::default();
    let client = vault(&env);
    let message = failure_of(|| {
        assert_panic_msg!(client.withdraw(&250), glob: "deposit of *");
    });
    assert!(
        message.contains("the panic message did not match"),
        "{message}"
    );
    assert!(
        message.contains("expected (glob): \"deposit of *\""),
        "{message}"
    );
    assert!(message.contains("origin: Soroban host error"), "{message}");
    assert!(
        message.contains("host error: Error(WasmVm, InvalidAction)"),
        "{message}"
    );
    assert!(
        message.contains("contract panic message: \"withdrawal of 250 exceeds the limit of 100\""),
        "{message}"
    );
    assert!(message.contains("actual (full host panic):"), "{message}");
}

/// The contract-side text only exists in the host's diagnostic event log.
/// With diagnostics off (`Env::host` is `#[doc(hidden)]` testutils API, used
/// here only to simulate that configuration) the host error is all that is
/// left, and the failure says the contract message is unavailable rather than
/// matching it against an empty string.
#[test]
fn without_diagnostics_only_the_host_error_is_available() {
    let env = Env::default();
    env.host()
        .set_diagnostic_level(DiagnosticLevel::None)
        .unwrap();
    let client = vault(&env);

    let captured = assert_panic_msg!(client.withdraw(&250), "Error(WasmVm, InvalidAction)");
    assert!(captured.contract_panics.is_empty());

    let message = failure_of(|| {
        assert_panic_msg!(client.withdraw(&250), "exceeds the limit");
    });
    assert!(
        message.contains("contract panic message: unavailable"),
        "{message}"
    );
}

// ---------------------------------------------------------------- failures

#[test]
fn mismatch_reports_expected_and_actual() {
    let message = failure_of(|| {
        assert_panic_msg!(panic!("insufficient balance: 3"), regex: r"overflow \d+");
    });
    assert!(
        message.contains("the panic message did not match"),
        "{message}"
    );
    assert!(
        message.contains("expected (regex): \"overflow \\\\d+\""),
        "{message}"
    );
    assert!(message.contains("origin: Rust panic"), "{message}");
    assert!(message.contains("| insufficient balance: 3"), "{message}");
}

#[test]
fn unexpected_success_is_reported() {
    let message = failure_of(|| {
        assert_panic_msg!(1 + 1, "anything");
    });
    assert!(
        message.contains("the invocation completed without panicking"),
        "{message}"
    );
    assert!(message.contains("(contains): \"anything\""), "{message}");
}

#[test]
fn successful_contract_call_is_reported_as_unexpected_success() {
    let env = Env::default();
    let client = vault(&env);
    let message = failure_of(|| {
        assert_panic_msg!(client.withdraw(&50), "exceeds");
    });
    assert!(message.contains("completed without panicking"), "{message}");
}

#[test]
fn non_string_payload_is_reported_not_matched_as_empty() {
    let message = failure_of(|| {
        // An empty substring would match any text; it must not match a
        // payload that carries none.
        assert_panic_msg!(std::panic::panic_any(42u32), "");
    });
    assert!(
        message.contains("payload is neither a String nor a &str"),
        "{message}"
    );
}

#[test]
fn invalid_regex_fails_before_running_the_invocation() {
    let runs = Cell::new(0);
    let message = failure_of(|| {
        assert_panic_msg!(
            {
                runs.set(runs.get() + 1);
                panic!("never reached")
            },
            regex: "unclosed ("
        );
    });
    assert!(message.contains("regex pattern is invalid"), "{message}");
    assert_eq!(runs.get(), 0);
}

#[test]
fn the_invocation_runs_exactly_once_on_match() {
    let runs = Cell::new(0);
    assert_panic_msg!(
        {
            runs.set(runs.get() + 1);
            panic!("run {}", runs.get())
        },
        glob: "run ?"
    );
    assert_eq!(runs.get(), 1);
}

#[test]
fn the_invocation_runs_exactly_once_on_mismatch_and_on_success() {
    let runs = Cell::new(0);
    let _ = failure_of(|| {
        assert_panic_msg!(
            {
                runs.set(runs.get() + 1);
                panic!("first")
            },
            "second"
        );
    });
    assert_eq!(runs.get(), 1);

    let calls = Cell::new(0);
    let _ = failure_of(|| {
        assert_panic_msg!(calls.set(calls.get() + 1), "x");
    });
    assert_eq!(calls.get(), 1);
}

#[test]
fn check_function_returns_typed_failures() {
    let pattern = PanicPattern::glob("a*");
    assert_eq!(pattern.mode(), MatchMode::Glob);

    let ok = check_panic_msg(capture_panic(|| panic!("abc")), &pattern).unwrap();
    assert_eq!(ok.raw, "abc");

    match check_panic_msg(capture_panic(|| ()), &pattern) {
        Err(PanicMsgFailure::UnexpectedSuccess { mode, pattern }) => {
            assert_eq!(mode, MatchMode::Glob);
            assert_eq!(pattern, "a*");
        }
        other => panic!("unexpected {other:?}"),
    }

    match check_panic_msg(capture_panic(|| panic!("xyz")), &pattern) {
        Err(PanicMsgFailure::Mismatch { captured, .. }) => assert_eq!(captured.raw, "xyz"),
        other => panic!("unexpected {other:?}"),
    }
}
