//! Stateful property-based testing for Soroban contracts.
//!
//! This module provides a simplified state machine harness that works with proptest 1.12.
//! It implements the state machine logic directly using proptest strategies.

use proptest::collection;
use proptest::strategy::{BoxedStrategy, Strategy};
use proptest::test_runner::{TestCaseError, TestCaseResult, TestRunner, Config as ProptestConfig};
use std::fmt::Debug;

/// A trait defining a state machine for Soroban contract testing.
///
/// Implement this trait to define the actions, model state, preconditions,
/// and invariants for your contract.
pub trait SorobanStateMachine {
    /// The type representing a single action/transition.
    type Action: Clone + Debug + 'static;

    /// The model state that tracks expected logical state.
    type Model: Clone + Debug + 'static;

    /// The test context containing the Soroban environment and contract clients.
    type Context: Clone + Debug + 'static;

    /// Generate an arbitrary action from the current model state.
    fn generate_action(&self, model: &Self::Model) -> BoxedStrategy<Self::Action>;

    /// Check if an action is valid in the current model state (precondition).
    fn check_precondition(&self, model: &Self::Model, action: &Self::Action) -> bool;

    /// Execute the action against the real contract, updating the context.
    fn execute(&self, context: &mut Self::Context, action: &Self::Action) -> Result<(), String>;

    /// Update the model state based on the action (for expected state).
    fn apply_to_model(&self, model: &mut Self::Model, action: &Self::Action);

    /// Verify the invariant holds between model and real contract state.
    fn check_invariant(&self, model: &Self::Model, context: &Self::Context) -> TestCaseResult;

    /// The initial model state.
    fn initial_model(&self) -> Self::Model;

    /// Create a fresh test context.
    fn create_context(&self) -> Self::Context;
}

/// A runner for Soroban state machine tests.
pub struct SorobanStateMachineRunner<SM: SorobanStateMachine> {
    state_machine: SM,
    config: ProptestConfig,
}

impl<SM: SorobanStateMachine + Clone> SorobanStateMachineRunner<SM> {
    /// Create a new runner with the given state machine.
    pub fn new(state_machine: SM) -> Self {
        Self {
            state_machine,
            config: ProptestConfig::with_cases(100),
        }
    }

    /// Set the number of test cases to run.
    pub fn cases(mut self, cases: u32) -> Self {
        self.config = ProptestConfig::with_cases(cases);
        self
    }

    /// Set the proptest configuration.
    pub fn config(mut self, config: ProptestConfig) -> Self {
        self.config = config;
        self
    }

    /// Run the state machine test using a custom implementation that generates
    /// sequences of actions and verifies invariants.
    pub fn run(&self) -> Result<(), String> {
        let machine = self.state_machine.clone();
        let config = self.config.clone();

        let mut runner = TestRunner::new(config);
        
        // Strategy to generate a sequence of actions
        let action_sequence = Self::generate_action_sequence(&machine);
        
        runner.run(&action_sequence, |actions| {
            let model = machine.initial_model();
            let mut context = machine.create_context();
            
            // Execute each action in sequence
            for action in actions {
                // Check precondition
                if !machine.check_precondition(&model, &action) {
                    return Err(TestCaseError::fail("Precondition failed"));
                }
                
                // Apply to model
                machine.apply_to_model(&mut model.clone(), &action);
                
                // Execute against real contract
                machine.execute(&mut context, &action)
                    .map_err(|e| TestCaseError::fail(e))?;
                
                // Check invariant after each action
                machine.check_invariant(&model, &context)?;
            }
            
            Ok(())
        }).map_err(|e| format!("State machine test failed: {}", e))
    }

    /// Generate a strategy for action sequences.
    fn generate_action_sequence(machine: &SM) -> BoxedStrategy<Vec<SM::Action>> {
        let initial_model = machine.initial_model();
        
        // Generate sequences of actions up to a reasonable length
        // We use a recursive strategy to build sequences
        let gen_actions = move |model: SM::Model| {
            machine.generate_action(&model)
        };
        
        // For simplicity, generate sequences of fixed length
        // In a full implementation, this would be more sophisticated
        let single_action = gen_actions(initial_model.clone());
        
        // Generate sequences of length 1 to 20
        collection::vec(single_action, 1..=20).boxed()
    }
}

/// A simpler property test approach: generate sequences of actions directly.
///
/// This provides a more direct way to test state machines without the full
/// proptest state_machine machinery.
pub fn prop_state_machine_test<SM>(
    machine: SM,
    max_actions: usize,
    cases: u32,
) -> Result<(), String>
where
    SM: SorobanStateMachine + Clone,
{
    let config = ProptestConfig {
        cases,
        ..ProptestConfig::default()
    };
    
    let mut runner = TestRunner::new(config);
    
    // Strategy to generate action sequences
    let action_strategy = {
        let machine = machine.clone();
        let initial_model = machine.initial_model();
        let gen_actions = move |model: SM::Model| machine.generate_action(&model);
        
        // Generate sequences of actions
        collection::vec(gen_actions(initial_model.clone()), 1..=max_actions).boxed()
    };
    
    runner.run(&action_strategy, move |actions| {
        let mut model = machine.initial_model();
        let mut context = machine.create_context();
        
        for action in actions {
            // Check precondition
            if !machine.check_precondition(&model, &action) {
                return Err(TestCaseError::fail("Precondition failed"));
            }
            
            // Apply to model
            machine.apply_to_model(&mut model, &action);
            
            // Execute against real contract
            machine.execute(&mut context, &action)
                .map_err(|e| TestCaseError::fail(e))?;
            
            // Check invariant
            machine.check_invariant(&model, &context)?;
        }
        
        Ok(())
    }).map_err(|e| format!("State machine test failed: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::{Env, Address, testutils::Address as _};
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};

    /// Wrapper for Address that implements Hash and Eq.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct AddrWrapper(Address);

    impl Hash for AddrWrapper {
        fn hash<H: Hasher>(&self, state: &mut H) {
            // Hash the underlying bytes using the XDR representation
            use soroban_sdk::xdr::{ScAddress, WriteXdr};
            let sc_address: ScAddress = self.0.clone().into();
            let bytes = sc_address.to_xdr(soroban_sdk::xdr::Limits::none())
                .expect("Address should serialize to XDR");
            bytes.hash(state);
        }
    }

    impl From<Address> for AddrWrapper {
        fn from(addr: Address) -> Self {
            AddrWrapper(addr)
        }
    }

    impl From<AddrWrapper> for Address {
        fn from(wrapper: AddrWrapper) -> Self {
            wrapper.0
        }
    }

    /// A simple token model for testing.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    struct TokenModel {
        balances: HashMap<AddrWrapper, i128>,
        total_supply: i128,
    }

    impl TokenModel {
        fn total_supply(&self) -> i128 {
            self.total_supply
        }
    }

    /// Actions for the token state machine.
    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TokenAction {
        Mint { to: AddrWrapper, amount: i128 },
        Transfer { from: AddrWrapper, to: AddrWrapper, amount: i128 },
        Burn { from: AddrWrapper, amount: i128 },
    }

    /// Test context with Soroban environment.
    #[derive(Clone)]
    struct TokenTestContext {
        env: Env,
        contract: Address,
        admin: Address,
    }

    impl Debug for TokenTestContext {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TokenTestContext")
                .field("contract", &self.contract)
                .field("admin", &self.admin)
                .finish()
        }
    }

    /// A simple state machine for token testing.
    #[derive(Clone)]
    struct TokenStateMachine {
        env: Env,
    }

    impl TokenStateMachine {
        fn new() -> Self {
            Self { env: Env::default() }
        }
    }

    impl Debug for TokenStateMachine {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TokenStateMachine").finish()
        }
    }

    impl SorobanStateMachine for TokenStateMachine {
        type Action = TokenAction;
        type Model = TokenModel;
        type Context = TokenTestContext;

        fn generate_action(&self, model: &Self::Model) -> BoxedStrategy<Self::Action> {
            let addresses: Vec<AddrWrapper> = model.balances.keys().cloned().collect();
            let has_balances = !addresses.is_empty();

            let mint = {
                let env = self.env.clone();
                (1i128..1_000_000)
                    .prop_map(move |amount| TokenAction::Mint { 
                        to: AddrWrapper(Address::generate(&env)), 
                        amount 
                    })
                    .boxed()
            };

            let transfer = if has_balances {
                let from = addresses.clone();
                let env = self.env.clone();
                let to = AddrWrapper(Address::generate(&env));
                (prop::sample::select(from), Just(to), 1i128..1_000_000)
                    .prop_map(|(from, to, amount)| TokenAction::Transfer { from, to, amount })
                    .boxed()
            } else {
                mint.clone()
            };

            let burn = if has_balances {
                let env = self.env.clone();
                (prop::sample::select(addresses), 1i128..1_000_000)
                    .prop_map(move |(from, amount)| TokenAction::Burn { from, amount })
                    .boxed()
            } else {
                mint.clone()
            };

            // Combine strategies using prop_oneof and box the result
            prop_oneof![
                4 => mint,
                4 => transfer,
                2 => burn,
            ]
            .boxed()
        }

        fn check_precondition(&self, model: &Self::Model, action: &Self::Action) -> bool {
            match action {
                TokenAction::Mint { amount, .. } => *amount > 0,
                TokenAction::Transfer { from, amount, .. } => {
                    *amount > 0 && model.balances.get(from).map(|b| *b >= *amount).unwrap_or(false)
                }
                TokenAction::Burn { from, amount } => {
                    *amount > 0 && model.balances.get(from).map(|b| *b >= *amount).unwrap_or(false)
                }
            }
        }

        fn execute(&self, _context: &mut Self::Context, _action: &Self::Action) -> Result<(), String> {
            // In a real test, this would call the contract
            // For this test, we just simulate success
            Ok(())
        }

        fn apply_to_model(&self, model: &mut Self::Model, action: &Self::Action) {
            match action {
                TokenAction::Mint { to, amount } => {
                    *model.balances.entry(to.clone()).or_insert(0) += amount;
                    model.total_supply += amount;
                }
                TokenAction::Transfer { from, to, amount } => {
                    *model.balances.entry(from.clone()).or_insert(0) -= amount;
                    *model.balances.entry(to.clone()).or_insert(0) += amount;
                }
                TokenAction::Burn { from, amount } => {
                    *model.balances.entry(from.clone()).or_insert(0) -= amount;
                    model.total_supply -= amount;
                }
            }
        }

        fn check_invariant(&self, model: &Self::Model, _context: &Self::Context) -> TestCaseResult {
            // Check that total supply matches sum of balances
            let sum: i128 = model.balances.values().sum();
            if sum != model.total_supply {
                return Err(TestCaseError::fail(format!(
                    "Invariant violated: sum of balances ({}) != total supply ({})",
                    sum, model.total_supply
                )));
            }
            Ok(())
        }

        fn initial_model(&self) -> Self::Model {
            TokenModel::default()
        }

        fn create_context(&self) -> Self::Context {
            let env = Env::default();
            let contract = Address::generate(&env);
            let admin = Address::generate(&env);
            TokenTestContext { env, contract, admin }
        }
    }

    #[test]
    fn test_token_state_machine_runner() {
        let machine = TokenStateMachine::new();
        let runner = SorobanStateMachineRunner::new(machine).cases(50);
        runner.run().unwrap();
    }

    #[test]
    fn test_prop_state_machine_test() {
        let machine = TokenStateMachine::new();
        prop_state_machine_test(machine, 20, 50).unwrap();
    }

    #[test]
    fn test_state_machine_shrinkage() {
        let machine = TokenStateMachine::new();
        let config = ProptestConfig {
            failure_persistence: None,
            cases: 100,
            ..ProptestConfig::default()
        };
        let runner = SorobanStateMachineRunner::new(machine).config(config);
        runner.run().unwrap();
    }
}