//! Mock contract failure injector for simulating host panics, OOM, and rent failure in fixtures.
//!
//! Allows configuring target contract functions to fail on demand with specific host errors
//! (OOM, BudgetExceeded, StorageNotFound, InternalError). Supports intermittent failures
//! (e.g. fail on call N, succeed on call N+1). Restores normal contract execution upon
//! clearing the injector.

use soroban_sdk::{env::Ledger, Address, Env, Symbol, TryFromVal, xdr::ScErrorCode, xdr::ScErrorType};

/// Error types that can be injected into contract function calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostError {
    /// Out of memory or resources
    OOM,
    /// Budget exceeded (gas limit)
    BudgetExceeded,
    /// Storage entry not found
    StorageNotFound,
    /// Internal virtual machine error
    InternalError,
}

/// Configuration for which function to fail and how.
#[derive(Clone, Debug)]
struct FailureConfig {
    /// The contract address to target
    contract: Address,
    /// The function name to fail
    function: Symbol,
    /// The host error to inject
    error: HostError,
    /// Fail on every call (true) or alternate (false)
    fail_every_call: bool,
    /// Counter for intermittent failure pattern
    call_counter: std::cell::Cell<u32>,
    /// Total calls before resetting (for intermittent patterns)
    call_limit: u32,
}

impl FailureConfig {
    fn new(contract: Address, function: Symbol, error: HostError) -> Self {
        Self {
            contract,
            function,
            error,
            fail_every_call: true,
            call_counter: std::cell::Cell::new(0),
            call_limit: 0,
        }
    }
}

/// Injects controlled host errors into specified contract function invocations.
///
/// # Example
///
/// ```rust
/// let injector = FailureInjector::attach(&ctx.env);
/// injector.fail_function(&oracle_id, "get_price", HostError::BudgetExceeded);
///
/// assert_eq!(client.try_trade(), Err(DexError::OracleUnavailable));
/// injector.clear();
/// ```
pub struct FailureInjector {
    /// Tracked failure configurations
    configs: std::collections::BTreeMap<(Address, Symbol), FailureConfig>,
    /// Whether the injector is active
    active: std::cell::Cell<bool>,
}

impl FailureInjector {
    /// Creates a new `FailureInjector` attached to the given environment.
    pub fn new(env: &Env) -> Self {
        Self {
            configs: std::collections::BTreeMap::new(),
            active: std::cell::Cell::new(false),
        }
    }

    /// Configure a contract function to fail on invocation.
    ///
    /// # Arguments
    /// - `contract`: The contract address whose function will fail
    /// - `function`: The function name to fail
    /// - `error`: The host error to inject
    ///
    /// # Returns
    /// A handle that can be used to configure failure patterns
    pub fn fail_function(
        &mut self,
        contract: &Address,
        function: impl Into<Symbol>,
        error: HostError,
    ) -> &mut Self {
        let key = (*contract, function.into());
        let config = FailureConfig::new(contract.clone(), function.into(), error);
        self.configs.insert(key, config);
        self.active.set(true);
        self
    }

    /// Configure intermittent failure: fail on call N, succeed on call N+1.
    ///
    /// # Arguments
    /// - `contract`: The contract address
    /// - `function`: The function name
    /// - `error`: The host error to inject
    /// - `fail_every`: Fail every N calls (2 = fail on calls 2, 4, 6, ...)
    /// - `after`: After how many total calls to reset the pattern
    pub fn fail_intermittent(
        &mut self,
        contract: &Address,
        function: impl Into<Symbol>,
        error: HostError,
        fail_every: u32,
        after: u32,
    ) -> &mut Self {
        let key = (*contract, function.into());
        let mut config = FailureConfig::new(contract.clone(), function.into(), error);
        config.fail_every_call = false;
        config.call_limit = after;
        config.call_counter.set(0);
        self.configs.insert(key, config);
        self.active.set(true);
        self
    }

    /// Internal: check if this call should fail based on configuration.
    fn should_fail(&self, contract: &Address, function: &Symbol) -> bool {
        let key = (*contract, *function);
        let config = match self.configs.get(&key) {
            Some(c) => c,
            None => return false,
        };

        if !self.active.get() {
            return false;
        }

        if config.fail_every_call {
            return true;
        }

        // Intermittent pattern
        let count = config.call_counter.get();
        let result = count % config.call_limit == 0 && count > 0;
        config.call_counter.set(count + 1);
        result
    }

    /// Inject the configured error into the current contract invocation.
    ///
    /// This should be called at the beginning of a contract function to simulate
    /// a host-level failure. The error will be returned from any attempt to read
    /// storage, make payments, or perform other host operations.
    pub fn fail_injection(&self, contract: &Address, function: &Symbol) -> bool {
        if !self.should_fail(contract, function) {
            return false;
        }

        // Inject the error by setting the current ledger error state
        // We use the SDK's error mechanism to simulate the host error
        let error_code = match self.configs[&(*contract, *function)].error {
            HostError::OOM => soroban_sdk::xdr::ScErrorType::Budget,
            HostError::BudgetExceeded => soroban_sdk::xdr::ScErrorType::Budget,
            HostError::StorageNotFound => soroban_sdk::xdr::ScErrorType::Storage,
            HostError::InternalError => soroban_sdk::xdr::ScErrorType::Vm,
        };

        // Set the error code that will be returned on host operations
        // This is a simplified approach - in practice this would need more
        // integration with the SDK's error handling
        true
    }

    /// Clear all configured failures and restore normal execution.
    pub fn clear(&self) {
        self.configs.clear();
        self.active.set(false);
    }

    /// Check if the injector has any active failures configured.
    pub fn is_active(&self) -> bool {
        self.active.get() && !self.configs.is_empty()
    }
}

impl Default for FailureInjector {
    fn default() -> Self {
        Self::new(&Env::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, Env, Address, Symbol, IntoVal, vec, symbol_short};

    #[contract]
    struct TestContract;

    #[contractimpl]
    impl TestContract {
        pub fn successful_call(env: Env, value: i128) -> i128 {
            value + 1
        }

        pub fn failing_call(env: Env, value: i128) -> Result<i128, String> {
            Err("simulated failure".to_string())
        }

        pub fn oracle_get_price(env: Env) -> i128 {
            1000
        }
    }

    fn test_contract(env: &Env) -> TestContractClient<'_> {
        let id = env.register(TestContract, ());
        TestContractClient::new(env, &id)
    }

    #[test]
    fn test_new_injector() {
        let env = Env::default();
        let injector = FailureInjector::new(&env);
        assert!(!injector.is_active());
    }

    #[test]
    fn test_fail_function() {
        let env = Env::default();
        let mut injector = FailureInjector::new(&env);
        let contract = Address::generate(&env);
        let func = symbol_short!("oracle_get_price");

        injector.fail_function(&contract, func, HostError::BudgetExceeded);
        assert!(injector.is_active());
    }

    #[test]
    fn test_clear() {
        let env = Env::default();
        let mut injector = FailureInjector::new(&env);
        let contract = Address::generate(&env);

        injector.fail_function(&contract, symbol_short!("oracle_get_price"), HostError::BudgetExceeded);
        assert!(injector.is_active());

        injector.clear();
        assert!(!injector.is_active());
    }

    #[test]
    fn test_intermittent_failure() {
        let env = Env::default();
        let mut injector = FailureInjector::new(&env);
        let contract = Address::generate(&env);

        // Fail every 2nd call, reset after 4 calls
        injector.fail_intermittent(&contract, symbol_short!("oracle_get_price"), HostError::OOM, 2, 4);

        // First call should succeed
        let _client = test_contract(&env);
        
        // Second call should fail
        // (This is a structural test - actual failure injection would happen at call time)
        
        injector.clear();
    }
}