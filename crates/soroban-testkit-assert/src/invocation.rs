//! Assertions for successful contract client invocations.

use soroban_testkit_core::error::{unwrap_decoded, ClientOutcome};

/// Returns a successful `try_*` contract client result or panics with its decoded error.
pub fn unwrap_client_call<T>(outcome: ClientOutcome<T>) -> T {
    unwrap_decoded(outcome)
}

/// Assert that a contract client invocation completes without an error.
///
/// Pass the generated client's `try_*` method so contract and host errors can
/// be decoded instead of being reported as a nested-result `unwrap` failure.
///
/// ```
/// # use soroban_testkit_assert::assert_no_contract_error;
/// # let result = Ok::<_, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>(
/// #     Ok::<_, soroban_sdk::ConversionError>(7u32)
/// # );
/// let value = assert_no_contract_error!(result);
/// assert_eq!(value, 7);
/// ```
#[macro_export]
macro_rules! assert_no_contract_error {
    ($call:expr $(,)?) => {
        $crate::invocation::unwrap_client_call($call)
    };
}

/// Assert that a contract client invocation succeeds and returns `expected`.
///
/// The `try_*` method is evaluated once. Contract and host failures use the
/// same decoded diagnostic as [`assert_no_contract_error!`].
///
/// ```
/// # use soroban_testkit_assert::assert_call_success_with_value;
/// # let result = Ok::<_, Result<soroban_sdk::Error, soroban_sdk::InvokeError>>(
/// #     Ok::<_, soroban_sdk::ConversionError>(7u32)
/// # );
/// assert_call_success_with_value!(result, 7);
/// ```
#[macro_export]
macro_rules! assert_call_success_with_value {
    ($call:expr, $expected:expr $(,)?) => {{
        let actual = $crate::invocation::unwrap_client_call($call);
        assert_eq!(actual, $expected);
    }};
}

#[cfg(test)]
mod tests {
    use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
    use soroban_sdk::{contract, contractimpl, Env};
    use soroban_testkit_core::error::ClientOutcome;

    #[contract]
    pub struct OutcomeContract;

    #[contractimpl]
    impl OutcomeContract {
        pub fn succeed(env: Env) -> u32 {
            let _ = env;
            7
        }
    }

    fn client(env: &Env) -> OutcomeContractClient<'_> {
        let id = env.register(OutcomeContract, ());
        OutcomeContractClient::new(env, &id)
    }

    #[test]
    fn assert_no_contract_error_returns_the_success_value() {
        let env = Env::default();
        let client = client(&env);

        assert_eq!(crate::assert_no_contract_error!(client.try_succeed()), 7);
    }

    #[test]
    fn assert_call_success_with_value_accepts_the_expected_value() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_call_success_with_value!(client.try_succeed(), 7);
    }

    #[test]
    #[should_panic(expected = "Soroban Error [storage/3]: MissingValue")]
    fn assert_no_contract_error_decodes_host_errors() {
        let outcome: ClientOutcome<u32> = Err(Ok(soroban_sdk::Error::from_type_and_code(
            ScErrorType::Storage,
            ScErrorCode::MissingValue,
        )));

        crate::assert_no_contract_error!(outcome);
    }

    #[test]
    #[should_panic(expected = "left: 7")]
    fn assert_call_success_with_value_reports_mismatches() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_call_success_with_value!(client.try_succeed(), 8);
    }
}
