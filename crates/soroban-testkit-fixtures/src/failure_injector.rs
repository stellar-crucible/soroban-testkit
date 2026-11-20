//! Controlled failures for mock dependency contracts (issue #109).
//!
//! Resilient contracts must survive a dependency that fails: an oracle that
//! runs out of budget, a vault whose storage expired, a token that traps. The
//! [`FailureInjector`] lets a test decide, per contract and per function, when
//! a dependency fails and with which host error — permanently, for the next
//! `n` calls, or only on the `k`-th call — and restores normal behaviour with
//! [`FailureInjector::clear`].
//!
//! # How the failure reaches the caller
//!
//! A test cannot rewrite another contract's code from the outside, so the
//! injection point lives in the mock dependency the test already registers:
//! its functions start with [`failure_point`]. When a rule matches, the mock
//! fails through the host (`Env::panic_with_error`) with the configured
//! [`InjectedFailure`], so the *calling* contract sees exactly what a real
//! host failure looks like — through `try_*` clients, `try_invoke_contract`,
//! or a trap that unwinds the whole transaction. A mock with no matching rule
//! runs normally, and `failure_point` costs nothing when no injector is
//! attached.
//!
//! ```ignore
//! #[contractimpl]
//! impl MockOracle {
//!     pub fn get_price(env: Env) -> i128 {
//!         failure_point(&env, "get_price");
//!         100
//!     }
//! }
//!
//! let injector = FailureInjector::attach(&env);
//! injector.fail_function(&oracle_id, "get_price", InjectedFailure::BudgetExceeded);
//! assert_eq!(dex.try_trade(), Err(Ok(DexError::OracleUnavailable)));
//! injector.clear();
//! ```
//!
//! # What the caller observes
//!
//! A [`InjectedFailure::Contract`] code reaches a `try_*` caller unchanged.
//! Every host-level failure reaches it as `Error(Context, InvalidAction)`,
//! because the host never forwards a callee's host error verbatim — the same
//! thing a caller sees when a real dependency traps. `StorageNotFound` raises a
//! genuine host trap (a TTL extension on a missing entry), so the diagnostic
//! log shows the real storage error.
//!
//! One deliberate difference from a live network: there, exhausting the CPU
//! or memory budget aborts the *whole transaction* and cannot be caught by any
//! caller. `BudgetExceeded` and `OutOfMemory` here fail only the dependency's
//! invocation, modelling a dependency that runs out and reports it, so the
//! caller's fallback path can be tested. To test transaction-wide exhaustion,
//! run the call under real limits (`NetworkProfile` or
//! `env.cost_estimate().budget().reset_limits`) instead.
//!
//! Rules are scoped to the test thread and to the injector that created them:
//! dropping the injector clears them, so one test cannot leak a failure into
//! the next.

use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
use soroban_sdk::{Address, Env, Error};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// A host failure a mock dependency can be made to raise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InjectedFailure {
    /// The memory budget is exhausted (`Error(Budget, ExceededLimit)` raised
    /// while allocating). Reported with the WASM VM error type to keep it
    /// distinguishable from a CPU budget breach.
    OutOfMemory,
    /// The CPU/instruction budget is exhausted: the "out of gas" case.
    BudgetExceeded,
    /// A storage read found no entry — what a dependency whose persistent
    /// entry was archived for unpaid rent reports.
    StorageNotFound,
    /// An internal host error (`Error(Context, InternalError)`).
    InternalError,
    /// A contract-defined error code, as a `#[contracterror]` would raise.
    Contract(u32),
}

impl InjectedFailure {
    /// The host [`Error`] this failure is raised as.
    pub fn to_error(self) -> Error {
        match self {
            InjectedFailure::OutOfMemory => {
                Error::from_type_and_code(ScErrorType::WasmVm, ScErrorCode::ExceededLimit)
            }
            InjectedFailure::BudgetExceeded => {
                Error::from_type_and_code(ScErrorType::Budget, ScErrorCode::ExceededLimit)
            }
            InjectedFailure::StorageNotFound => {
                Error::from_type_and_code(ScErrorType::Storage, ScErrorCode::MissingValue)
            }
            InjectedFailure::InternalError => {
                Error::from_type_and_code(ScErrorType::Context, ScErrorCode::InternalError)
            }
            InjectedFailure::Contract(code) => Error::from_contract_error(code),
        }
    }
}

/// When a rule fires, counted in calls to the target function since the rule
/// was installed (1-based).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Schedule {
    /// Every call fails until cleared: a permanent crash.
    Always,
    /// The next `n` calls fail, then the function recovers.
    FirstN(u32),
    /// Only call number `k` fails.
    OnCall(u32),
}

#[derive(Clone, Debug)]
struct Rule {
    owner: u64,
    failure: InjectedFailure,
    schedule: Schedule,
    calls: u32,
}

type Key = (String, String);

#[derive(Default)]
struct Registry {
    rules: BTreeMap<Key, Rule>,
    next_owner: u64,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = RefCell::new(Registry::default());
}

fn key(contract: &Address, function: &str) -> Key {
    (format!("{}", contract.to_string()), function.to_string())
}

/// Call at the top of a mock dependency's function. Fails the invocation
/// through the host when an injected rule matches the current contract and
/// `function`; otherwise returns and the function runs normally.
pub fn failure_point(env: &Env, function: &str) {
    let fire = REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.rules.is_empty() {
            return None;
        }
        let rule = registry
            .rules
            .get_mut(&key(&env.current_contract_address(), function))?;
        rule.calls += 1;
        let fails = match rule.schedule {
            Schedule::Always => true,
            Schedule::FirstN(n) => rule.calls <= n,
            Schedule::OnCall(k) => rule.calls == k,
        };
        fails.then_some(rule.failure)
    });
    match fire {
        None => {}
        // A genuine missing-entry trap: extending the TTL of a persistent key
        // that was never written makes the host itself raise
        // `Error(Storage, MissingValue)`, exactly as an archived or never-
        // created entry would.
        Some(InjectedFailure::StorageNotFound) => {
            let missing = soroban_sdk::Symbol::new(env, "__tk_injected_missing");
            env.storage().persistent().extend_ttl(&missing, 1, 1);
        }
        Some(failure) => env.panic_with_error(failure.to_error()),
    }
}

/// Configures which mock dependency calls fail. See the module docs.
pub struct FailureInjector {
    owner: u64,
    _env: Env,
    _not_send: Rc<()>,
}

impl FailureInjector {
    /// Attach an injector to `env`'s test thread. Rules it installs are
    /// removed by [`clear`](Self::clear) or when it is dropped.
    pub fn attach(env: &Env) -> Self {
        let owner = REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            registry.next_owner += 1;
            registry.next_owner
        });
        Self {
            owner,
            _env: env.clone(),
            _not_send: Rc::new(()),
        }
    }

    fn install(
        &self,
        contract: &Address,
        function: &str,
        failure: InjectedFailure,
        schedule: Schedule,
    ) {
        REGISTRY.with(|registry| {
            registry.borrow_mut().rules.insert(
                key(contract, function),
                Rule {
                    owner: self.owner,
                    failure,
                    schedule,
                    calls: 0,
                },
            );
        });
    }

    /// Every call to `contract.function` fails with `failure` until cleared.
    pub fn fail_function(&self, contract: &Address, function: &str, failure: InjectedFailure) {
        self.install(contract, function, failure, Schedule::Always);
    }

    /// The next `times` calls fail, then the function recovers on its own: a
    /// temporary outage.
    pub fn fail_times(
        &self,
        contract: &Address,
        function: &str,
        failure: InjectedFailure,
        times: u32,
    ) {
        self.install(contract, function, failure, Schedule::FirstN(times));
    }

    /// Only call number `call` (1-based) fails; the calls around it succeed.
    pub fn fail_on_call(
        &self,
        contract: &Address,
        function: &str,
        failure: InjectedFailure,
        call: u32,
    ) {
        self.install(contract, function, failure, Schedule::OnCall(call));
    }

    /// Calls the injection point has seen for `contract.function` since its
    /// rule was installed (0 when there is no rule). Counts failed calls too.
    pub fn calls(&self, contract: &Address, function: &str) -> u32 {
        REGISTRY.with(|registry| {
            registry
                .borrow()
                .rules
                .get(&key(contract, function))
                .map_or(0, |rule| rule.calls)
        })
    }

    /// Remove the rule for one function, restoring it.
    pub fn restore(&self, contract: &Address, function: &str) {
        REGISTRY.with(|registry| {
            let mut registry = registry.borrow_mut();
            let k = key(contract, function);
            if registry
                .rules
                .get(&k)
                .is_some_and(|rule| rule.owner == self.owner)
            {
                registry.rules.remove(&k);
            }
        });
    }

    /// Remove every rule this injector installed, restoring normal execution.
    pub fn clear(&self) {
        REGISTRY.with(|registry| {
            registry
                .borrow_mut()
                .rules
                .retain(|_, rule| rule.owner != self.owner);
        });
    }

    /// Whether this injector still has any rule installed.
    pub fn is_active(&self) -> bool {
        REGISTRY.with(|registry| {
            registry
                .borrow()
                .rules
                .values()
                .any(|rule| rule.owner == self.owner)
        })
    }
}

impl Drop for FailureInjector {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contracterror, contractimpl, Address, Env};

    #[contract]
    struct MockOracle;

    #[contractimpl]
    impl MockOracle {
        pub fn get_price(env: Env) -> i128 {
            failure_point(&env, "get_price");
            100
        }

        pub fn decimals(env: Env) -> u32 {
            failure_point(&env, "decimals");
            7
        }
    }

    #[contracterror]
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    #[repr(u32)]
    pub enum DexError {
        OracleUnavailable = 1,
    }

    /// The caller under test: it must turn a failing oracle into its own
    /// error instead of trapping.
    #[contract]
    struct Dex;

    #[contractimpl]
    impl Dex {
        pub fn quote(env: Env, oracle: Address, amount: i128) -> Result<i128, DexError> {
            match MockOracleClient::new(&env, &oracle).try_get_price() {
                Ok(Ok(price)) => Ok(amount * price),
                _ => Err(DexError::OracleUnavailable),
            }
        }

        /// No error handling: a failing oracle aborts the whole call.
        pub fn quote_unchecked(env: Env, oracle: Address, amount: i128) -> i128 {
            amount * MockOracleClient::new(&env, &oracle).get_price()
        }
    }

    struct Setup {
        env: Env,
        oracle: Address,
        dex_id: Address,
    }

    impl Setup {
        fn dex(&self) -> DexClient<'_> {
            DexClient::new(&self.env, &self.dex_id)
        }
    }

    fn setup() -> Setup {
        let env = Env::default();
        let oracle = env.register(MockOracle, ());
        let dex_id = env.register(Dex, ());
        Setup {
            env,
            oracle,
            dex_id,
        }
    }

    #[test]
    fn no_rule_means_normal_execution() {
        let s = setup();
        let _injector = FailureInjector::attach(&s.env);
        assert_eq!(s.dex().quote(&s.oracle, &2), 200);
    }

    #[test]
    fn caller_error_handling_catches_every_injected_failure() {
        for failure in [
            InjectedFailure::OutOfMemory,
            InjectedFailure::BudgetExceeded,
            InjectedFailure::StorageNotFound,
            InjectedFailure::InternalError,
            InjectedFailure::Contract(42),
        ] {
            let s = setup();
            let injector = FailureInjector::attach(&s.env);
            injector.fail_function(&s.oracle, "get_price", failure);
            assert_eq!(
                s.dex().try_quote(&s.oracle, &2),
                Err(Ok(DexError::OracleUnavailable)),
                "{failure:?} was not routed to the caller's error path"
            );
        }
    }

    #[test]
    fn the_failure_reaches_the_caller_as_the_configured_host_error() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        let client = MockOracleClient::new(&s.env, &s.oracle);

        injector.fail_function(&s.oracle, "get_price", InjectedFailure::Contract(42));
        assert_eq!(
            client.try_get_price(),
            Err(Ok(Error::from_contract_error(42)))
        );

        // The host does not pass a callee's host-level error through to its
        // caller verbatim: any non-contract failure of a sub-invocation is
        // reported as `Error(Context, InvalidAction)` — for a genuine trap
        // (the missing-storage case triggers a real one) and an injected one
        // alike, which is what makes the injection faithful.
        for failure in [
            InjectedFailure::OutOfMemory,
            InjectedFailure::BudgetExceeded,
            InjectedFailure::StorageNotFound,
            InjectedFailure::InternalError,
        ] {
            injector.fail_function(&s.oracle, "get_price", failure);
            assert_eq!(
                client.try_get_price(),
                Err(Ok(Error::from_type_and_code(
                    ScErrorType::Context,
                    ScErrorCode::InvalidAction
                ))),
                "{failure:?}"
            );
        }
    }

    #[test]
    fn an_unhandled_failure_aborts_the_caller() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        injector.fail_function(&s.oracle, "get_price", InjectedFailure::BudgetExceeded);
        assert!(s.dex().try_quote_unchecked(&s.oracle, &2).is_err());
    }

    #[test]
    fn rules_are_per_function() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        injector.fail_function(&s.oracle, "get_price", InjectedFailure::InternalError);
        assert_eq!(MockOracleClient::new(&s.env, &s.oracle).decimals(), 7);
    }

    #[test]
    fn temporary_failure_recovers_after_n_calls() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        injector.fail_times(&s.oracle, "get_price", InjectedFailure::BudgetExceeded, 2);
        assert!(s.dex().try_quote(&s.oracle, &1).is_err());
        assert!(s.dex().try_quote(&s.oracle, &1).is_err());
        assert_eq!(s.dex().quote(&s.oracle, &1), 100);
        assert_eq!(injector.calls(&s.oracle, "get_price"), 3);
    }

    #[test]
    fn intermittent_failure_on_call_n_only() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        injector.fail_on_call(&s.oracle, "get_price", InjectedFailure::StorageNotFound, 2);
        assert_eq!(s.dex().quote(&s.oracle, &1), 100);
        assert_eq!(
            s.dex().try_quote(&s.oracle, &1),
            Err(Ok(DexError::OracleUnavailable))
        );
        assert_eq!(s.dex().quote(&s.oracle, &1), 100);
    }

    #[test]
    fn clear_and_drop_restore_normal_execution() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        injector.fail_function(&s.oracle, "get_price", InjectedFailure::InternalError);
        assert!(injector.is_active());
        injector.clear();
        assert!(!injector.is_active());
        assert_eq!(s.dex().quote(&s.oracle, &3), 300);

        {
            let scoped = FailureInjector::attach(&s.env);
            scoped.fail_function(&s.oracle, "get_price", InjectedFailure::InternalError);
            assert!(s.dex().try_quote(&s.oracle, &1).is_err());
        }
        assert_eq!(
            s.dex().quote(&s.oracle, &1),
            100,
            "dropping the injector restores the mock"
        );
    }

    #[test]
    fn restore_removes_one_rule() {
        let s = setup();
        let injector = FailureInjector::attach(&s.env);
        injector.fail_function(&s.oracle, "get_price", InjectedFailure::InternalError);
        injector.fail_function(&s.oracle, "decimals", InjectedFailure::InternalError);
        injector.restore(&s.oracle, "get_price");
        assert_eq!(s.dex().quote(&s.oracle, &1), 100);
        assert!(MockOracleClient::new(&s.env, &s.oracle)
            .try_decimals()
            .is_err());
    }
}
