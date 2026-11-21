//! Asserting the exact error a contract call failed with.
//!
//! `#[should_panic]` passes for *any* panic, so a test meant to prove
//! "unauthorized callers are refused" keeps passing when the call starts
//! failing on a budget overflow instead. [`assert_contract_error!`] and
//! [`assert_host_error!`] take the `Result` a generated client's `try_*`
//! method returns and fail unless it carries exactly the error expected:
//!
//! ```rust
//! use soroban_sdk::{contract, contracterror, contractimpl, Env};
//! use soroban_testkit_assert::assert_contract_error;
//!
//! #[contracterror]
//! #[derive(Copy, Clone, Debug, Eq, PartialEq)]
//! #[repr(u32)]
//! pub enum TokenError {
//!     InsufficientBalance = 1,
//!     Unauthorized = 2,
//! }
//!
//! #[contract]
//! pub struct Token;
//!
//! #[contractimpl]
//! impl Token {
//!     pub fn transfer(_env: Env, amount: i128) -> Result<(), TokenError> {
//!         if amount > 100 {
//!             return Err(TokenError::InsufficientBalance);
//!         }
//!         Ok(())
//!     }
//! }
//!
//! # fn main() {
//! let env = Env::default();
//! let client = TokenClient::new(&env, &env.register(Token, ()));
//! assert_contract_error!(client.try_transfer(&1000), TokenError::InsufficientBalance);
//! # }
//! ```
//!
//! On a mismatch the failure names the expected variant, the variant (or
//! host error) actually received, and the code and category of both.

use std::fmt::{self, Debug};

use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
use soroban_sdk::{Error, InvokeError};
use soroban_testkit_core::error::DecodedError;

/// The `Result` shape every generated `try_*` client method returns:
/// `Ok(Ok(value))` on success, `Ok(Err(_))` when the value did not convert,
/// `Err(Ok(error))` for an error the client's error type can represent and
/// `Err(Err(invoke))` for one it cannot.
pub type TryCallResult<T, C, E> = Result<Result<T, C>, Result<E, InvokeError>>;

/// What a failed call actually carried, rendered for a diagnostic.
#[derive(Debug, Clone)]
pub struct ReceivedError {
    /// The `Debug` form of the typed error, e.g. `TokenError::Unauthorized`
    /// prints as `Unauthorized`.
    pub variant: String,
    /// The decoded code and category, when the host error is still known.
    pub decoded: Option<DecodedError>,
}

impl fmt::Display for ReceivedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.decoded {
            Some(decoded) => write!(
                f,
                "{} (code {}, category {})",
                self.variant, decoded.code, decoded.category
            ),
            None => f.write_str(&self.variant),
        }
    }
}

fn describe_typed<E: Debug + Clone + Into<Error>>(error: &E) -> ReceivedError {
    ReceivedError {
        variant: format!("{error:?}"),
        decoded: Some(DecodedError::from_error(&error.clone().into())),
    }
}

fn describe_invoke(error: &InvokeError) -> ReceivedError {
    match error {
        InvokeError::Contract(code) => ReceivedError {
            variant: format!(
                "contract error code {code}, which the client's error type has no variant for"
            ),
            decoded: Some(DecodedError::from_contract_code(*code)),
        },
        InvokeError::Abort => ReceivedError {
            variant: "InvokeError::Abort — a panic or host failure the client's error type cannot represent"
                .to_string(),
            decoded: None,
        },
    }
}

/// Check a `try_*` result against an expected `#[contracterror]` variant,
/// returning the diagnostic when it does not match.
///
/// This is the function [`assert_contract_error!`] calls; use it directly to
/// collect a mismatch without panicking.
pub fn check_contract_error<T, C, E>(
    outcome: TryCallResult<T, C, E>,
    expected: E,
) -> Result<(), String>
where
    T: Debug,
    C: Debug,
    E: Debug + Clone + PartialEq + Into<Error>,
{
    let wanted = describe_typed(&expected);
    let received = match outcome {
        Err(Ok(actual)) if actual == expected => return Ok(()),
        Err(Ok(actual)) => describe_typed(&actual),
        Err(Err(invoke)) => describe_invoke(&invoke),
        Ok(Ok(value)) => {
            return Err(format!(
                "assert_contract_error! failed: the call succeeded, but it was expected to fail\n  expected: {wanted}\n  returned: {value:?}"
            ))
        }
        Ok(Err(conversion)) => {
            return Err(format!(
                "assert_contract_error! failed: the call succeeded (its return value did not convert: {conversion:?}), but it was expected to fail\n  expected: {wanted}"
            ))
        }
    };
    Err(format!(
        "assert_contract_error! failed: the call failed with a different error\n  expected: {wanted}\n  received: {received}{}",
        decoded_line(&received)
    ))
}

fn decoded_line(received: &ReceivedError) -> String {
    match &received.decoded {
        Some(decoded) => format!("\n  decoded:  {decoded}"),
        None => String::new(),
    }
}

/// An expected host error: a whole category, an exact category and code, or
/// a concrete [`Error`] value.
pub trait HostErrorExpectation {
    /// Whether `error` satisfies the expectation.
    fn matches(&self, error: &Error) -> bool;
    /// How the expectation reads in a failure message.
    fn describe(&self) -> String;
}

impl HostErrorExpectation for Error {
    fn matches(&self, error: &Error) -> bool {
        self == error
    }

    fn describe(&self) -> String {
        DecodedError::from_error(self).to_string()
    }
}

impl HostErrorExpectation for ScErrorType {
    fn matches(&self, error: &Error) -> bool {
        error.is_type(*self)
    }

    fn describe(&self) -> String {
        format!("any {} error ({self:?})", category_name(*self))
    }
}

impl HostErrorExpectation for (ScErrorType, ScErrorCode) {
    fn matches(&self, error: &Error) -> bool {
        error.is_type(self.0) && error.is_code(self.1)
    }

    fn describe(&self) -> String {
        DecodedError::from_error(&Error::from_type_and_code(self.0, self.1)).to_string()
    }
}

fn category_name(ty: ScErrorType) -> &'static str {
    // Decode a representative error of the category to reuse the core table.
    let probe = if ty == ScErrorType::Contract {
        Error::from_contract_error(0)
    } else {
        Error::from_type_and_code(ty, ScErrorCode::InternalError)
    };
    DecodedError::from_error(&probe).category
}

/// Check a `try_*` result against an expected host error, returning the
/// diagnostic when it does not match.
///
/// The client must be able to carry the host error: when its error type is
/// [`soroban_sdk::Error`] (any function without a `Result<_, ContractError>`
/// return type), every failure arrives intact. A `#[contracterror]` client
/// only represents contract codes, so a host failure arrives as
/// [`InvokeError::Abort`] with its category lost — the diagnostic says so.
///
/// Errors the host treats as unrecoverable — budget exhaustion above all —
/// are escalated to a panic even through `try_*`, so they never reach this
/// check; catch those with `#[should_panic(expected = "Error(Budget, ExceededLimit)")]`.
pub fn check_host_error<T, C, E, X>(
    outcome: TryCallResult<T, C, E>,
    expected: X,
) -> Result<(), String>
where
    T: Debug,
    C: Debug,
    E: Debug + Clone + Into<Error>,
    X: HostErrorExpectation,
{
    let wanted = expected.describe();
    let actual: Error = match outcome {
        Err(Ok(error)) => error.into(),
        Err(Err(InvokeError::Contract(code))) => Error::from_contract_error(code),
        Err(Err(InvokeError::Abort)) => {
            return Err(format!(
                "assert_host_error! failed: the call aborted, but the client's error type cannot say why\n  expected: {wanted}\n  received: InvokeError::Abort\n  hint:     a #[contracterror] client drops host errors; call env.try_invoke_contract::<_, soroban_sdk::Error>(..) to keep the category and code"
            ))
        }
        Ok(Ok(value)) => {
            return Err(format!(
                "assert_host_error! failed: the call succeeded, but it was expected to fail\n  expected: {wanted}\n  returned: {value:?}"
            ))
        }
        Ok(Err(conversion)) => {
            return Err(format!(
                "assert_host_error! failed: the call succeeded (its return value did not convert: {conversion:?}), but it was expected to fail\n  expected: {wanted}"
            ))
        }
    };
    if expected.matches(&actual) {
        return Ok(());
    }
    let decoded = DecodedError::from_error(&actual);
    Err(format!(
        "assert_host_error! failed: the call failed with a different error\n  expected: {wanted}\n  received: {decoded} (code {}, category {})",
        decoded.code, decoded.category
    ))
}

/// Assert that a `try_*` client call failed with exactly the given
/// `#[contracterror]` variant.
///
/// The expected value is any type the client's error converts to
/// [`soroban_sdk::Error`] from — the contract's own error enum, or a raw
/// `soroban_sdk::Error::from_contract_error(code)`. Fails, printing the
/// expected and received variant with their code and category, when the
/// call failed differently or succeeded.
///
/// ```rust
/// # use soroban_sdk::{contracterror, Error, InvokeError};
/// # use soroban_testkit_assert::assert_contract_error;
/// #[contracterror]
/// #[derive(Copy, Clone, Debug, Eq, PartialEq)]
/// #[repr(u32)]
/// pub enum VaultError {
///     Locked = 4,
/// }
///
/// let outcome: Result<Result<(), soroban_sdk::ConversionError>, Result<VaultError, InvokeError>> =
///     Err(Ok(VaultError::Locked));
/// assert_contract_error!(outcome, VaultError::Locked);
/// ```
#[macro_export]
macro_rules! assert_contract_error {
    ($call:expr, $expected:expr $(,)?) => {
        if let Err(message) = $crate::contract_error::check_contract_error($call, $expected) {
            panic!("{}", message);
        }
    };
}

/// Assert that a `try_*` client call failed with a host error.
///
/// Three forms:
///
/// * `assert_host_error!(call, ScErrorType::Budget)` — any error of the
///   category;
/// * `assert_host_error!(call, ScErrorType::Storage, ScErrorCode::MissingValue)`
///   — an exact category and code;
/// * `assert_host_error!(call, error)` — equal to a concrete
///   [`soroban_sdk::Error`], including one built with
///   `Error::from_contract_error`.
///
/// ```rust
/// # use soroban_sdk::{xdr::{ScErrorCode, ScErrorType}, Error, InvokeError};
/// # use soroban_testkit_assert::assert_host_error;
/// let outcome: Result<Result<u32, soroban_sdk::ConversionError>, Result<Error, InvokeError>> =
///     Err(Ok(Error::from_type_and_code(ScErrorType::Budget, ScErrorCode::ExceededLimit)));
/// assert_host_error!(outcome, ScErrorType::Budget, ScErrorCode::ExceededLimit);
/// ```
#[macro_export]
macro_rules! assert_host_error {
    ($call:expr, $ty:expr, $code:expr $(,)?) => {
        if let Err(message) = $crate::contract_error::check_host_error($call, ($ty, $code)) {
            panic!("{}", message);
        }
    };
    ($call:expr, $expected:expr $(,)?) => {
        if let Err(message) = $crate::contract_error::check_host_error($call, $expected) {
            panic!("{}", message);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{
        contract, contracterror, contractimpl, panic_with_error, symbol_short, ConversionError, Env,
    };

    #[contracterror]
    #[derive(Copy, Clone, Debug, Eq, PartialEq)]
    #[repr(u32)]
    pub enum TokenError {
        InsufficientBalance = 1,
        Unauthorized = 2,
    }

    #[contract]
    pub struct Token;

    #[contractimpl]
    impl Token {
        pub fn transfer(_env: Env, amount: i128) -> Result<i128, TokenError> {
            match amount {
                a if a < 0 => Err(TokenError::Unauthorized),
                a if a > 100 => Err(TokenError::InsufficientBalance),
                a => Ok(a),
            }
        }

        /// Raises a code the enum does not name.
        pub fn rogue(env: Env) -> Result<(), TokenError> {
            panic_with_error!(&env, Error::from_contract_error(99))
        }

        /// Extends the TTL of a key that was never written, which the host
        /// refuses.
        pub fn missing(env: Env) -> u32 {
            env.storage()
                .persistent()
                .extend_ttl(&symbol_short!("none"), 1, 1);
            0
        }

        pub fn checked(env: Env, ok: bool) -> u32 {
            if !ok {
                panic_with_error!(&env, TokenError::Unauthorized);
            }
            1
        }
    }

    fn client(env: &Env) -> TokenClient<'_> {
        TokenClient::new(env, &env.register(Token, ()))
    }

    #[test]
    fn a_matching_contract_error_passes() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_contract_error!(client.try_transfer(&1000), TokenError::InsufficientBalance);
        crate::assert_contract_error!(client.try_transfer(&-1), TokenError::Unauthorized);
    }

    #[test]
    fn a_raw_contract_error_value_matches_too() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_contract_error!(client.try_checked(&false), Error::from_contract_error(2));
    }

    #[test]
    fn a_mismatched_variant_names_both_variants_codes_and_categories() {
        let env = Env::default();
        let client = client(&env);

        let message =
            check_contract_error(client.try_transfer(&-1), TokenError::InsufficientBalance)
                .unwrap_err();

        assert!(
            message.contains("failed with a different error"),
            "{message}"
        );
        assert!(
            message.contains("expected: InsufficientBalance (code 1, category contract)"),
            "{message}"
        );
        assert!(
            message.contains("received: Unauthorized (code 2, category contract)"),
            "{message}"
        );
    }

    #[test]
    #[should_panic(expected = "received: Unauthorized (code 2, category contract)")]
    fn the_macro_panics_with_the_mismatch() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_contract_error!(client.try_transfer(&-1), TokenError::InsufficientBalance);
    }

    #[test]
    fn an_unexpected_success_fails_cleanly() {
        let env = Env::default();
        let client = client(&env);

        let message =
            check_contract_error(client.try_transfer(&5), TokenError::InsufficientBalance)
                .unwrap_err();

        assert!(message.contains("the call succeeded"), "{message}");
        assert!(message.contains("returned: 5"), "{message}");
    }

    #[test]
    #[should_panic(expected = "the call succeeded, but it was expected to fail")]
    fn the_macro_panics_on_unexpected_success() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_contract_error!(client.try_transfer(&5), TokenError::Unauthorized);
    }

    #[test]
    fn a_code_outside_the_enum_is_reported_as_unnamed() {
        let env = Env::default();
        let client = client(&env);

        let message =
            check_contract_error(client.try_rogue(), TokenError::Unauthorized).unwrap_err();

        assert!(
            message.contains(
                "contract error code 99, which the client's error type has no variant for"
            ),
            "{message}"
        );
        assert!(message.contains("Soroban Error [99]"), "{message}");
    }

    #[test]
    fn a_conversion_failure_counts_as_success() {
        let outcome: TryCallResult<u32, ConversionError, TokenError> = Ok(Err(ConversionError));

        let message = check_contract_error(outcome, TokenError::Unauthorized).unwrap_err();

        assert!(message.contains("did not convert"), "{message}");
    }

    #[test]
    fn a_host_error_matches_by_category_and_code() {
        let env = Env::default();
        let client = client(&env);

        // A host failure inside a native (non-Wasm) test contract surfaces as
        // the host catching a panic in the contract frame: context/InvalidAction.
        crate::assert_host_error!(
            client.try_missing(),
            ScErrorType::Context,
            ScErrorCode::InvalidAction
        );
        crate::assert_host_error!(client.try_missing(), ScErrorType::Context);
        crate::assert_host_error!(
            env.try_invoke_contract::<u32, Error>(
                &client.address,
                &symbol_short!("nope"),
                soroban_sdk::Vec::new(&env),
            ),
            ScErrorType::Context
        );
    }

    #[test]
    fn budget_exhaustion_is_told_apart_from_other_failures() {
        // The host escalates budget exhaustion to a panic even through a
        // `try_*` call, so the outcome is built by hand here.
        let exhausted = || -> TryCallResult<u32, ConversionError, Error> {
            Err(Ok(Error::from_type_and_code(
                ScErrorType::Budget,
                ScErrorCode::ExceededLimit,
            )))
        };

        crate::assert_host_error!(exhausted(), ScErrorType::Budget, ScErrorCode::ExceededLimit);
        let wrong = check_host_error(exhausted(), ScErrorType::Auth).unwrap_err();
        assert!(wrong.contains("category budget"), "{wrong}");
        assert!(wrong.contains("expected: any auth error"), "{wrong}");
    }

    #[test]
    fn a_host_error_mismatch_prints_the_decoded_error() {
        let env = Env::default();
        let client = client(&env);

        let message = check_host_error(
            client.try_missing(),
            (ScErrorType::Storage, ScErrorCode::MissingValue),
        )
        .unwrap_err();

        assert!(
            message.contains("expected: Soroban Error [storage/3]"),
            "{message}"
        );
        assert!(
            message.contains("received: Soroban Error [context/6]: InvalidAction"),
            "{message}"
        );
        assert!(message.contains("(code 6, category context)"), "{message}");
    }

    #[test]
    fn assert_host_error_accepts_a_concrete_error_value() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_host_error!(
            client.try_checked(&false),
            Error::from_contract_error(TokenError::Unauthorized as u32)
        );
    }

    #[test]
    fn a_contracterror_client_loses_the_host_category_and_says_so() {
        let outcome: TryCallResult<u32, ConversionError, TokenError> = Err(Err(InvokeError::Abort));

        let message = check_host_error(outcome, ScErrorType::Budget).unwrap_err();

        assert!(message.contains("InvokeError::Abort"), "{message}");
        assert!(message.contains("try_invoke_contract"), "{message}");
    }

    #[test]
    #[should_panic(expected = "assert_host_error! failed: the call succeeded")]
    fn assert_host_error_fails_on_unexpected_success() {
        let env = Env::default();
        let client = client(&env);

        crate::assert_host_error!(client.try_checked(&true), ScErrorType::Storage);
    }
}
