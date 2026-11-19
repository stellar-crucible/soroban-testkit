//! Contract invocation under real Soroban host resource limits.
//!
//! The report distinguishes successful calls, contract errors, host errors and
//! actual budget exhaustion. Resource costs and host diagnostics are captured
//! before the SDK test budget is reset to its default limits.
//!
//! ```
//! use soroban_sdk::{
//!     contract, contractimpl, symbol_short, Env, Val, Vec,
//! };
//! use soroban_testkit_fixtures::GasExhaustionHarness;
//!
//! mod counter {
//!     use soroban_sdk::{contract, contractimpl, symbol_short, Env};
//!
//!     #[contract]
//!     pub struct Counter;
//!
//!     #[contractimpl]
//!     impl Counter {
//!         pub fn increment(env: Env) {
//!             let key = symbol_short!("count");
//!             let count: u32 = env.storage().persistent().get(&key).unwrap_or(0);
//!             env.storage().persistent().set(&key, &(count + 1));
//!         }
//!     }
//! }
//!
//! let env = Env::default();
//! let id = env.register(counter::Counter, ());
//! let report = GasExhaustionHarness::cpu_250k().invoke(
//!     &env,
//!     &id,
//!     "increment",
//!     Vec::<Val>::new(&env),
//! );
//! assert!(report.is_success(), "{}", report.diagnostic());
//! ```

use soroban_sdk::{
    testutils::{budget::Budget, Logs as _},
    xdr::{ScErrorCode, ScErrorType},
    Address, ConversionError, Env, Error, InvokeError, Symbol, Val, Vec,
};
use soroban_testkit_core::budget::BudgetSnapshot;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// CPU and memory limits applied to one contract invocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionBudget {
    pub cpu_limit: u64,
    pub memory_limit: u64,
}

/// Which metered resource reached its configured limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExhaustedResource {
    Cpu,
    Memory,
    CpuAndMemory,
    Unclassified,
}

/// The result of invoking a contract under a restricted host budget.
#[derive(Debug)]
pub enum ExecutionStatus {
    Succeeded(Val),
    BudgetExhausted {
        resource: ExhaustedResource,
        /// The error value returned by the SDK, when one is available.
        error: Option<Error>,
    },
    ContractError(Error),
    HostError(Error),
    InvocationError(InvokeError),
    ConversionError(ConversionError),
    InvocationPanicked(String),
}

/// Outcome and diagnostics captured for one restricted invocation.
#[derive(Debug)]
pub struct ExecutionReport {
    pub budget: ExecutionBudget,
    pub metered: BudgetSnapshot,
    pub status: ExecutionStatus,
    /// The contract address supplied to the invocation.
    pub contract_id: Address,
    /// The function name supplied to the invocation.
    pub function: String,
    /// Host diagnostic events exposed by SDK v28's `Env::logs().all()`.
    pub diagnostics: std::vec::Vec<String>,
}

impl ExecutionReport {
    pub fn is_success(&self) -> bool {
        matches!(&self.status, ExecutionStatus::Succeeded(_))
    }

    pub fn is_budget_exhausted(&self) -> bool {
        matches!(&self.status, ExecutionStatus::BudgetExhausted { .. })
    }

    /// A concise diagnostic suitable for assertion messages.
    pub fn diagnostic(&self) -> String {
        let mut output = format!(
            "execution status: {:?}\ninvocation: contract={:?} function={}\nconfigured limits: cpu={} memory={}\nmetered: cpu={} memory={}",
            self.status,
            self.contract_id,
            self.function,
            self.budget.cpu_limit,
            self.budget.memory_limit,
            self.metered.cpu_insns,
            self.metered.mem_bytes,
        );
        if self.diagnostics.is_empty() {
            output.push_str("\nhost diagnostics: none recorded");
        } else {
            output.push_str("\nhost diagnostics:");
            for entry in &self.diagnostics {
                output.push_str("\n  ");
                output.push_str(entry);
            }
        }
        output
    }
}

/// Executes contract invocations using Soroban's real, metered host budget.
///
/// The configured limits apply only to the invocation. After collecting its
/// result, metered cost and host diagnostics, the harness resets the env budget
/// to the SDK's default limits so callers can inspect storage or continue their
/// test. Soroban host transactions provide rollback when an invocation fails;
/// the harness never attempts to restore contract state itself.
///
/// Native test contracts are not metered like deployed Wasm contracts: VM
/// execution and Wasm instruction costs are absent. For representative
/// resource use, register or upload the Wasm contract under test. The host
/// reports invocation-level errors and diagnostics, not the Rust source line
/// or loop iteration that caused an exhaustion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GasExhaustionHarness {
    budget: ExecutionBudget,
}

impl Default for GasExhaustionHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl GasExhaustionHarness {
    /// Construct a harness with a 250,000 CPU instruction limit and unlimited
    /// test-host memory.
    pub fn new() -> Self {
        Self::cpu_250k()
    }

    pub fn cpu_50k() -> Self {
        Self::with_limits(50_000, u64::MAX)
    }

    pub fn cpu_100k() -> Self {
        Self::with_limits(100_000, u64::MAX)
    }

    pub fn cpu_250k() -> Self {
        Self::with_limits(250_000, u64::MAX)
    }

    pub fn with_limits(cpu_limit: u64, memory_limit: u64) -> Self {
        Self {
            budget: ExecutionBudget {
                cpu_limit,
                memory_limit,
            },
        }
    }

    pub fn budget(&self) -> ExecutionBudget {
        self.budget
    }

    /// Invoke `function` on `contract_id` with the supplied Soroban values.
    ///
    /// Contract errors, host errors, invocation aborts, value-conversion
    /// failures and actual budget exhaustion are represented separately in
    /// [`ExecutionStatus`].
    pub fn invoke(
        &self,
        env: &Env,
        contract_id: &Address,
        function: &str,
        args: Vec<Val>,
    ) -> ExecutionReport {
        let mut budget = env.cost_estimate().budget();
        budget.reset_limits(self.budget.cpu_limit, self.budget.memory_limit);
        budget.reset_tracker();

        let invocation = catch_unwind(AssertUnwindSafe(|| {
            env.try_invoke_contract::<Val, Error>(contract_id, &Symbol::new(env, function), args)
        }));

        let budget = env.cost_estimate().budget();
        let metered = BudgetSnapshot {
            cpu_insns: budget.cpu_instruction_cost(),
            mem_bytes: budget.memory_bytes_cost(),
        };
        let mut diagnostics = env.logs().all();
        let status = match invocation {
            Ok(invocation) => classify_invocation(invocation, metered, self.budget),
            Err(payload) => {
                let message = panic_message(payload.as_ref());
                diagnostics.push(format!("invocation panic: {message}"));
                let resource = exhausted_resource(metered, self.budget);
                if resource == ExhaustedResource::Unclassified {
                    ExecutionStatus::InvocationPanicked(message)
                } else {
                    ExecutionStatus::BudgetExhausted {
                        resource,
                        error: None,
                    }
                }
            }
        };

        let mut budget: Budget = env.cost_estimate().budget();
        budget.reset_default();

        ExecutionReport {
            budget: self.budget,
            metered,
            status,
            contract_id: contract_id.clone(),
            function: function.to_owned(),
            diagnostics,
        }
    }
}

fn classify_invocation(
    invocation: Result<Result<Val, ConversionError>, Result<Error, InvokeError>>,
    metered: BudgetSnapshot,
    budget: ExecutionBudget,
) -> ExecutionStatus {
    match invocation {
        Ok(Ok(value)) => ExecutionStatus::Succeeded(value),
        Ok(Err(error)) => ExecutionStatus::ConversionError(error),
        Err(Ok(error))
            if error.is_type(ScErrorType::Budget)
                && error.get_code() == ScErrorCode::ExceededLimit as u32 =>
        {
            ExecutionStatus::BudgetExhausted {
                resource: exhausted_resource(metered, budget),
                error: Some(error),
            }
        }
        Err(Ok(error)) if error.is_type(ScErrorType::Contract) => {
            ExecutionStatus::ContractError(error)
        }
        Err(Ok(error))
            if exhausted_resource(metered, budget) != ExhaustedResource::Unclassified =>
        {
            ExecutionStatus::BudgetExhausted {
                resource: exhausted_resource(metered, budget),
                error: Some(error),
            }
        }
        Err(Ok(error)) => ExecutionStatus::HostError(error),
        Err(Err(InvokeError::Abort))
            if exhausted_resource(metered, budget) != ExhaustedResource::Unclassified =>
        {
            ExecutionStatus::BudgetExhausted {
                resource: exhausted_resource(metered, budget),
                error: None,
            }
        }
        Err(Err(error)) => ExecutionStatus::InvocationError(error),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "non-string panic payload".to_string()
    }
}

fn exhausted_resource(metered: BudgetSnapshot, budget: ExecutionBudget) -> ExhaustedResource {
    match (
        metered.cpu_insns > budget.cpu_limit,
        metered.mem_bytes > budget.memory_limit,
    ) {
        (true, false) => ExhaustedResource::Cpu,
        (false, true) => ExhaustedResource::Memory,
        (true, true) => ExhaustedResource::CpuAndMemory,
        (false, false) => ExhaustedResource::Unclassified,
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionStatus, ExhaustedResource, GasExhaustionHarness};
    use soroban_sdk::{
        contract, contracterror, contractimpl, symbol_short, xdr::ScErrorType, Bytes, Env, IntoVal,
        Val,
    };
    use soroban_testkit_core::storage::StorageSnapshot;

    #[contracterror]
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    #[repr(u32)]
    pub enum ContractFailure {
        Rejected = 1,
    }

    #[contract]
    struct ResourceContract;

    #[contractimpl]
    impl ResourceContract {
        pub fn process(env: Env, iterations: u32) {
            env.storage()
                .persistent()
                .set(&symbol_short!("marker"), &iterations);
            for index in 0..iterations {
                env.storage().persistent().set(&index, &index);
            }
        }

        pub fn allocate(env: Env, size: u32) {
            env.storage()
                .persistent()
                .set(&symbol_short!("marker"), &size);
            let bytes = std::vec![0; size as usize];
            let _ = Bytes::from_slice(&env, &bytes);
        }

        pub fn fail(_env: Env) -> Result<(), ContractFailure> {
            Err(ContractFailure::Rejected)
        }
    }

    fn args(env: &Env, value: u32) -> soroban_sdk::Vec<Val> {
        soroban_sdk::vec![env, value.into_val(env)]
    }

    #[test]
    fn a_safe_batch_commits_state_and_reports_metered_cost() {
        let env = Env::default();
        let id = env.register(ResourceContract, ());

        let report = GasExhaustionHarness::with_limits(50_000_000, 50_000_000).invoke(
            &env,
            &id,
            "process",
            args(&env, 3),
        );

        assert!(report.is_success(), "{}", report.diagnostic());
        assert!(report.metered.cpu_insns > 0);
        assert!(!StorageSnapshot::capture(&env, &id).is_empty());
    }

    #[test]
    fn cpu_exhaustion_is_host_enforced_and_rolls_back_partial_writes() {
        let env = Env::default();
        let id = env.register(ResourceContract, ());
        let calibration = GasExhaustionHarness::with_limits(50_000_000, 50_000_000).invoke(
            &env,
            &id,
            "process",
            args(&env, 0),
        );
        assert!(calibration.is_success(), "{}", calibration.diagnostic());
        let before = StorageSnapshot::capture(&env, &id);

        let cpu_limit = calibration.metered.cpu_insns.saturating_add(50_000);
        let report = GasExhaustionHarness::with_limits(cpu_limit, u64::MAX).invoke(
            &env,
            &id,
            "process",
            args(&env, 10_000),
        );

        assert!(
            matches!(
                &report.status,
                ExecutionStatus::BudgetExhausted {
                    resource: ExhaustedResource::Cpu,
                    ..
                }
            ),
            "{}",
            report.diagnostic()
        );
        assert!(report.metered.cpu_insns > cpu_limit);
        assert!(report.diagnostic().contains("function=process"));
        assert!(report.diagnostic().contains("metered: cpu="));
        assert_eq!(StorageSnapshot::capture(&env, &id), before);
    }

    #[test]
    fn memory_exhaustion_is_host_enforced_and_rolls_back_state() {
        let env = Env::default();
        let id = env.register(ResourceContract, ());
        let calibration = GasExhaustionHarness::with_limits(u64::MAX, 50_000_000).invoke(
            &env,
            &id,
            "allocate",
            args(&env, 0),
        );
        assert!(calibration.is_success(), "{}", calibration.diagnostic());
        let before = StorageSnapshot::capture(&env, &id);

        let memory_limit = calibration.metered.mem_bytes.saturating_add(256);
        let report = GasExhaustionHarness::with_limits(u64::MAX, memory_limit).invoke(
            &env,
            &id,
            "allocate",
            args(&env, 100_000),
        );

        assert!(
            matches!(
                &report.status,
                ExecutionStatus::BudgetExhausted {
                    resource: ExhaustedResource::Memory,
                    ..
                }
            ),
            "{}",
            report.diagnostic()
        );
        assert!(report.metered.mem_bytes > memory_limit);
        assert_eq!(StorageSnapshot::capture(&env, &id), before);
    }

    #[test]
    fn successful_invocation_persists_all_changes() {
        let env = Env::default();
        let id = env.register(ResourceContract, ());
        let before = StorageSnapshot::capture(&env, &id);

        let report = GasExhaustionHarness::with_limits(50_000_000, 50_000_000).invoke(
            &env,
            &id,
            "process",
            args(&env, 2),
        );
        let after = StorageSnapshot::capture(&env, &id);

        assert!(report.is_success(), "{}", report.diagnostic());
        assert_ne!(after, before);
    }

    #[test]
    fn preset_and_custom_budget_boundaries_are_exposed() {
        assert_eq!(GasExhaustionHarness::cpu_50k().budget().cpu_limit, 50_000);
        assert_eq!(GasExhaustionHarness::cpu_100k().budget().cpu_limit, 100_000);
        assert_eq!(GasExhaustionHarness::cpu_250k().budget().cpu_limit, 250_000);
        assert_eq!(
            GasExhaustionHarness::with_limits(123, 456).budget(),
            super::ExecutionBudget {
                cpu_limit: 123,
                memory_limit: 456
            }
        );
    }

    #[test]
    fn contract_errors_are_not_reported_as_budget_exhaustion() {
        let env = Env::default();
        let id = env.register(ResourceContract, ());

        let report = GasExhaustionHarness::with_limits(50_000_000, 50_000_000).invoke(
            &env,
            &id,
            "fail",
            soroban_sdk::Vec::<Val>::new(&env),
        );

        match &report.status {
            ExecutionStatus::ContractError(error) => {
                assert!(error.is_type(ScErrorType::Contract));
            }
            _ => panic!("expected a contract error, got {}", report.diagnostic()),
        }
        assert!(!report.is_budget_exhausted());
    }
}
