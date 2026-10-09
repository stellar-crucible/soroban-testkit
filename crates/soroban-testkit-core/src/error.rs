use std::collections::BTreeMap;
use std::fmt;

use soroban_sdk::{
    xdr::{ScErrorCode, ScErrorType},
    ConversionError, Error, InvokeError,
};

/// Which part of Soroban raised an error.
pub const CATEGORY_CONTRACT: &str = "contract";

const HOST_CATEGORIES: [(ScErrorType, &str, &str); 10] = [
    (
        ScErrorType::Contract,
        CATEGORY_CONTRACT,
        "contract-specific, user-defined codes",
    ),
    (
        ScErrorType::WasmVm,
        "wasm_vm",
        "errors while interpreting WASM bytecode",
    ),
    (
        ScErrorType::Context,
        "context",
        "errors in the contract's host context",
    ),
    (
        ScErrorType::Storage,
        "storage",
        "errors accessing host storage",
    ),
    (
        ScErrorType::Object,
        "object",
        "errors working with host objects",
    ),
    (
        ScErrorType::Crypto,
        "crypto",
        "errors in cryptographic operations",
    ),
    (
        ScErrorType::Events,
        "events",
        "errors while emitting events",
    ),
    (
        ScErrorType::Budget,
        "budget",
        "errors relating to budget limits",
    ),
    (
        ScErrorType::Value,
        "value",
        "errors working with host values or ScVals",
    ),
    (
        ScErrorType::Auth,
        "auth",
        "errors from the authentication subsystem",
    ),
];

const HOST_CODES: [(ScErrorCode, &str, &str); 10] = [
    (
        ScErrorCode::ArithDomain,
        "ArithDomain",
        "some arithmetic was undefined (overflow or division by zero)",
    ),
    (
        ScErrorCode::IndexBounds,
        "IndexBounds",
        "something was indexed beyond its bounds",
    ),
    (
        ScErrorCode::InvalidInput,
        "InvalidInput",
        "the caller provided otherwise-bad data",
    ),
    (
        ScErrorCode::MissingValue,
        "MissingValue",
        "a required value was not provided",
    ),
    (
        ScErrorCode::ExistingValue,
        "ExistingValue",
        "a value was provided where none is allowed",
    ),
    (
        ScErrorCode::ExceededLimit,
        "ExceededLimit",
        "a gas or size limit was hit",
    ),
    (
        ScErrorCode::InvalidAction,
        "InvalidAction",
        "the data was valid but the requested action was not",
    ),
    (
        ScErrorCode::InternalError,
        "InternalError",
        "the host detected an error in its own logic",
    ),
    (
        ScErrorCode::UnexpectedType,
        "UnexpectedType",
        "a value's type was not what was expected",
    ),
    (
        ScErrorCode::UnexpectedSize,
        "UnexpectedSize",
        "a value's size was not what was expected",
    ),
];

/// An error whose numeric code has been turned into words.
#[derive(Debug, Clone)]
pub struct DecodedError {
    pub code: u32,
    /// `"contract"`, one of the host categories (`"storage"`, `"auth"`, …) or
    /// `"unknown"` when the bits do not name a Soroban error.
    pub category: &'static str,
    pub message: String,
    pub context: Option<String>,
}

impl DecodedError {
    /// Decode a `Result::Err` coming back from a contract call.
    ///
    /// Host categories are described from the protocol tables. A contract error
    /// keeps its code as a number unless the caller supplies an
    /// [`ErrorRegistry`] naming it — the SDK gives the host no way to know what
    /// an application's own codes mean.
    pub fn from_error(error: &Error) -> Self {
        Self::from_error_with(error, &ErrorRegistry::new())
    }

    /// Decode with `registry` consulted for contract codes.
    pub fn from_error_with(error: &Error, registry: &ErrorRegistry) -> Self {
        let category = HOST_CATEGORIES
            .iter()
            .find(|(ty, _, _)| error.is_type(*ty))
            .map(|(_, name, _)| *name);
        let code = error.get_code();

        match category {
            Some(name) if name == CATEGORY_CONTRACT => registry.decoded_contract(code),
            Some(name) => Self {
                code,
                category: name,
                message: host_message(name, code),
                context: None,
            },
            None => Self {
                code,
                category: "unknown",
                message: format!(
                    "the error does not carry a known Soroban category ({code:#010x})"
                ),
                context: None,
            },
        }
    }

    /// Decode a bare `#[contracterror]` code, as read from `Error::get_code()`.
    pub fn from_contract_code(code: u32) -> Self {
        ErrorRegistry::new().decoded_contract(code)
    }

    /// The code as the host renders it: `category/code` for host errors, the
    /// bare number for contract errors.
    pub fn label(&self) -> String {
        if self.category == CATEGORY_CONTRACT || self.category == "unknown" {
            self.code.to_string()
        } else {
            format!("{}/{}", self.category, self.code)
        }
    }
}

impl fmt::Display for DecodedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Soroban Error [{}]: {}", self.label(), self.message)?;
        if let Some(ctx) = &self.context {
            write!(f, " (context: {})", ctx)?;
        }
        Ok(())
    }
}

/// Contract-authored error codes and what they mean in this application.
///
/// `#[contracterror]` gives the host an integer and nothing else, so a test
/// reading a failure has to supply the vocabulary itself.
#[derive(Debug, Clone, Default)]
pub struct ErrorRegistry {
    entries: BTreeMap<u32, String>,
}

impl ErrorRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `code` as `message`, overwriting any earlier name for it.
    pub fn register(mut self, code: u32, message: impl Into<String>) -> Self {
        self.entries.insert(code, message.into());
        self
    }

    pub fn lookup(&self, code: u32) -> Option<&str> {
        self.entries.get(&code).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Decode a contract code against this registry.
    pub fn decoded_contract(&self, code: u32) -> DecodedError {
        let message = match self.lookup(code) {
            Some(named) => named.to_string(),
            None => "unregistered contract error — add it to an ErrorRegistry to read it as text"
                .to_string(),
        };
        DecodedError {
            code,
            category: CATEGORY_CONTRACT,
            message,
            context: None,
        }
    }
}

/// The `Result` a generated client's `try_*` method returns in SDK v28: the
/// converted Ok value, or the error the contract raised.
///
/// The nesting is the SDK's, not Testkit's: the outer `Err` means the call did
/// not produce a value, and inside it `Ok(error)` is a contract error while
/// `Err(invoke)` is the invocation itself failing.
pub type ClientOutcome<T> = Result<Result<T, ConversionError>, Result<Error, InvokeError>>;

/// Unwrap a `try_*` client call, panicking with the decoded error.
///
/// A plain `unwrap()` on the same value prints nested `Result` debug output;
/// this prints the sentence a test author has to act on.
pub fn unwrap_decoded<T>(outcome: ClientOutcome<T>) -> T {
    unwrap_decoded_with(outcome, &ErrorRegistry::new())
}

/// Same as [`unwrap_decoded`], with `registry` naming the contract's own codes.
pub fn unwrap_decoded_with<T>(outcome: ClientOutcome<T>, registry: &ErrorRegistry) -> T {
    match outcome {
        Ok(Ok(value)) => value,
        Ok(Err(conversion)) => {
            panic!("the returned value did not convert into the expected type: {conversion:?}")
        }
        Err(Ok(error)) => panic!("{}", DecodedError::from_error_with(&error, registry)),
        Err(Err(invoke)) => panic!("the invocation failed before returning: {invoke:?}"),
    }
}

fn host_message(category: &'static str, code: u32) -> String {
    let described = HOST_CODES
        .iter()
        .find(|(known, _, _)| *known as u32 == code)
        .map(|(_, label, detail)| format!("{} — {}", label, detail));
    let detail = match described {
        Some(text) => text,
        None => format!("host error code {} has no known meaning", code),
    };
    let summary = HOST_CATEGORIES
        .iter()
        .find(|(_, name, _)| *name == category)
        .map(|(_, _, summary)| *summary)
        .unwrap_or("unknown category");
    format!("{} ({})", detail, summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contracterror, contractimpl, xdr::ScErrorCode, Env};

    #[contracterror]
    #[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
    #[repr(u32)]
    pub enum Failure {
        InsufficientBalance = 101,
        Paused = 102,
    }

    #[contract]
    pub struct Gate;

    #[contractimpl]
    impl Gate {
        pub fn fail(env: Env) -> Result<u32, Error> {
            let _ = env;
            Err(Error::from_contract_error(
                Failure::InsufficientBalance as u32,
            ))
        }
    }

    fn gate(env: &Env) -> GateClient<'_> {
        let id = env.register(Gate, ());
        GateClient::new(env, &id)
    }

    fn contract_error(code: u32) -> Error {
        Error::from_contract_error(code)
    }

    #[test]
    fn every_host_category_is_covered() {
        for name in [
            "contract", "wasm_vm", "context", "storage", "object", "crypto", "events", "budget",
            "value", "auth",
        ] {
            assert!(
                HOST_CATEGORIES.iter().any(|(_, n, _)| *n == name),
                "category {} has no description",
                name
            );
        }
    }

    #[test]
    fn every_host_code_is_covered() {
        for code in 0..10u32 {
            let text = host_message("storage", code);
            assert!(
                !text.contains("has no known meaning"),
                "code {} is unmapped: {}",
                code,
                text
            );
        }
    }

    #[test]
    fn decode_reads_the_category_of_a_host_error() {
        let error = Error::from_type_and_code(ScErrorType::Storage, ScErrorCode::MissingValue);
        let decoded = DecodedError::from_error(&error);

        assert_eq!(decoded.category, "storage");
        assert_eq!(decoded.code, ScErrorCode::MissingValue as u32);
        assert_eq!(decoded.label(), "storage/3");
        assert_eq!(
            decoded.to_string(),
            "Soroban Error [storage/3]: MissingValue — a required value was not provided (errors accessing host storage)"
        );
    }

    #[test]
    fn decode_labels_budget_exhaustion_distinctly() {
        let decoded = DecodedError::from_error(&Error::from_type_and_code(
            ScErrorType::Budget,
            ScErrorCode::ExceededLimit,
        ));

        assert_eq!(decoded.category, "budget");
        assert!(decoded.to_string().contains("gas or size limit was hit"));
    }

    #[test]
    fn contract_codes_stay_numbers_without_a_registry() {
        let decoded = DecodedError::from_error(&contract_error(101));

        assert_eq!(decoded.category, "contract");
        assert_eq!(decoded.code, 101);
        assert_eq!(decoded.label(), "101");
        assert!(decoded.to_string().starts_with("Soroban Error [101]:"));
        assert!(decoded.to_string().contains("unregistered contract error"));
    }

    #[test]
    fn a_registry_turns_a_contract_code_into_words() {
        let registry = ErrorRegistry::new()
            .register(Failure::InsufficientBalance as u32, "Insufficient balance")
            .register(Failure::Paused as u32, "Contract is paused");

        let decoded = DecodedError::from_error_with(&contract_error(102), &registry);

        assert_eq!(decoded.category, "contract");
        assert_eq!(
            decoded.to_string(),
            "Soroban Error [102]: Contract is paused"
        );
        assert_eq!(registry.lookup(101), Some("Insufficient balance"));
        assert_eq!(registry.lookup(999), None);
    }

    #[test]
    fn registry_replaces_an_earlier_name_for_the_same_code() {
        let registry = ErrorRegistry::new()
            .register(1, "first")
            .register(1, "second");

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.lookup(1), Some("second"));
    }

    #[test]
    fn unknown_codes_fall_back_instead_of_panicking() {
        let decoded = DecodedError::from_contract_code(4242);

        assert_eq!(decoded.category, "contract");
        assert!(decoded.to_string().contains("unregistered"));

        let text = host_message("storage", 77);
        assert!(text.contains("host error code 77 has no known meaning"));
    }

    #[test]
    fn errors_from_a_real_contract_call_decode() {
        let env = Env::default();
        let client = gate(&env);
        let registry = ErrorRegistry::new()
            .register(Failure::InsufficientBalance as u32, "Insufficient balance");

        let error = match client.try_fail() {
            Ok(value) => panic!("expected the call to fail, got {:?}", value),
            Err(Err(invoke)) => panic!("the invocation itself failed: {:?}", invoke),
            Err(Ok(error)) => error,
        };

        assert_eq!(
            DecodedError::from_error_with(&error, &registry).to_string(),
            "Soroban Error [101]: Insufficient balance"
        );
        let bare = DecodedError::from_error(&error).to_string();
        assert!(
            bare.starts_with("Soroban Error [101]: unregistered"),
            "{}",
            bare
        );
    }

    #[test]
    #[should_panic(expected = "Soroban Error [budget/5]: ExceededLimit")]
    fn unwrap_decoded_panics_with_the_decoded_message() {
        let error = Error::from_type_and_code(
            soroban_sdk::xdr::ScErrorType::Budget,
            ScErrorCode::ExceededLimit,
        );

        let outcome: ClientOutcome<u32> = Err(Ok(error));

        unwrap_decoded(outcome);
    }

    #[test]
    fn unwrap_decoded_passes_a_success_through() {
        let outcome: ClientOutcome<u32> = Ok(Ok(7));

        assert_eq!(unwrap_decoded(outcome), 7);
    }

    #[test]
    #[should_panic(expected = "Soroban Error [101]: Insufficient balance")]
    fn unwrap_decoded_with_panics_with_the_registered_words() {
        let env = Env::default();
        let client = gate(&env);
        let registry = ErrorRegistry::new().register(101, "Insufficient balance");

        unwrap_decoded_with(client.try_fail(), &registry);
    }

    #[test]
    fn display_includes_code_and_message() {
        let err = DecodedError {
            code: 12,
            category: CATEGORY_CONTRACT,
            message: "Insufficient balance".to_string(),
            context: None,
        };
        assert_eq!(err.to_string(), "Soroban Error [12]: Insufficient balance");
    }

    #[test]
    fn display_appends_context_when_present() {
        let err = DecodedError {
            code: 7,
            category: CATEGORY_CONTRACT,
            message: "Not authorized".to_string(),
            context: Some("transfer".to_string()),
        };
        assert_eq!(
            err.to_string(),
            "Soroban Error [7]: Not authorized (context: transfer)"
        );
    }

    #[test]
    fn empty_message_still_renders_the_code() {
        let err = DecodedError {
            code: 0,
            category: CATEGORY_CONTRACT,
            message: String::new(),
            context: None,
        };
        assert_eq!(err.to_string(), "Soroban Error [0]: ");
    }

    #[test]
    fn clone_produces_an_independent_copy() {
        let err = DecodedError {
            code: 1,
            category: CATEGORY_CONTRACT,
            message: "first".to_string(),
            context: Some("ctx".to_string()),
        };
        let mut copy = err.clone();
        copy.message = "second".to_string();

        assert_eq!(err.message, "first");
        assert_eq!(copy.message, "second");
    }

    #[test]
    fn an_empty_registry_is_the_default_start() {
        let registry = ErrorRegistry::new();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }
}
