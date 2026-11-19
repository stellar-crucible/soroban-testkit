use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, Address, Env,
};

const LOCKED: soroban_sdk::Symbol = symbol_short!("locked");

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ReentrancyError {
    Locked = 1,
}

#[contract]
pub struct ReentrancyTarget;

#[contractimpl]
impl ReentrancyTarget {
    pub fn execute(env: Env, receiver: Address, panic_after_callback: bool) {
        assert!(!is_locked(&env), "target is already locked");
        env.storage().instance().set(&LOCKED, &true);
        ReentrancyReceiverClient::new(&env, &receiver)
            .callback(&env.current_contract_address(), &panic_after_callback);
        env.storage().instance().set(&LOCKED, &false);
    }

    pub fn reenter(env: Env) -> Result<(), ReentrancyError> {
        if is_locked(&env) {
            return Err(ReentrancyError::Locked);
        }
        Ok(())
    }

    pub fn is_locked(env: Env) -> bool {
        is_locked(&env)
    }
}

#[contract]
pub struct ReentrancyReceiver;

#[contractimpl]
impl ReentrancyReceiver {
    pub fn callback(env: Env, target: Address, panic_after_callback: bool) {
        let recursive_result = ReentrancyTargetClient::new(&env, &target).try_reenter();
        match recursive_result {
            Ok(Err(_)) => {}
            _ => panic!("recursive invocation was not rejected by the target guard"),
        }
        if panic_after_callback {
            panic!("simulated receiver panic after re-entrant call");
        }
    }
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReentrancyGuardHarness {
    pub env: Env,
    pub target: Address,
    pub receiver: Address,
}

impl ReentrancyGuardHarness {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        let target = env.register(ReentrancyTarget, ());
        let receiver = env.register(ReentrancyReceiver, ());
        Self {
            env,
            target,
            receiver,
        }
    }

    /// Executes the target and catches a host error when the receiver panics.
    pub fn execute(&self, panic_after_callback: bool) -> bool {
        ReentrancyTargetClient::new(&self.env, &self.target)
            .try_execute(&self.receiver, &panic_after_callback)
            .is_ok()
    }

    pub fn lock_is_set(&self) -> bool {
        ReentrancyTargetClient::new(&self.env, &self.target).is_locked()
    }
}

impl Default for ReentrancyGuardHarness {
    fn default() -> Self {
        Self::new()
    }
}

fn is_locked(env: &Env) -> bool {
    env.storage()
        .instance()
        .get(&LOCKED)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::ReentrancyGuardHarness;

    #[test]
    fn callback_reentry_is_rejected_and_success_clears_the_lock() {
        let harness = ReentrancyGuardHarness::new();

        assert!(harness.execute(false));
        assert!(!harness.lock_is_set());
    }

    #[test]
    fn a_caught_receiver_panic_rolls_back_the_guard_lock() {
        let harness = ReentrancyGuardHarness::new();

        assert!(!harness.execute(true));
        assert!(!harness.lock_is_set());
    }

    #[test]
    fn a_fresh_harness_starts_unlocked() {
        let harness = ReentrancyGuardHarness::default();
        assert!(!harness.lock_is_set());
    }
}
