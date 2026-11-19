//! Helpers for exercising persistent-entry archival and restoration.

use soroban_sdk::{
    testutils::{storage::Persistent as _, Ledger as _},
    Env, Val,
};

/// Advances a test ledger past an entry's TTL and restores the archived entry.
///
/// The simulator stores real host state and advances past its TTL. Since SDK 28
/// auto-restores archived entries in tests, it tracks the archived phase and
/// reports the legacy storage error until [`StateEvictionSimulator::restore`]
/// invokes the host's restoration path.
pub struct StateEvictionSimulator {
    env: Env,
    contract: soroban_sdk::Address,
    key: Val,
    value: Val,
    ttl: u32,
    archived: core::cell::Cell<bool>,
}

impl StateEvictionSimulator {
    /// Creates an isolated persistent value for a contract.
    pub fn new(env: &Env, contract: &soroban_sdk::Address, key: Val, value: Val) -> Self {
        Self {
            env: env.clone(),
            contract: contract.clone(),
            key,
            value,
            ttl: 1,
            archived: core::cell::Cell::new(false),
        }
    }

    /// Stores the original value with a short persistent TTL and returns it.
    pub fn store(&mut self) -> u32 {
        self.env.ledger().set_min_persistent_entry_ttl(1);
        self.env.ledger().set_max_entry_ttl(100);
        // Protocol 22 emulates the pre-automatic-restore behavior, allowing a
        // test to observe the archived read failure before restoring at v23.
        self.env.ledger().set_protocol_version(22);
        let key = self.key;
        let value = self.value;
        let ttl = self.env.as_contract(&self.contract, || {
            self.env.storage().persistent().set(&key, &value);
            self.env
                .storage()
                .persistent()
                .extend_ttl(&key, 0, self.ttl);
            self.env.storage().persistent().get_ttl(&key)
        });
        self.ttl = ttl;
        self.archived.set(false);
        self.ttl
    }

    /// Moves to the first ledger after the entry's live-until ledger.
    pub fn advance_past_ttl(&self) -> u32 {
        let next = self.env.ledger().sequence().saturating_add(self.ttl + 1);
        self.env.ledger().set_sequence_number(next);
        self.archived.set(true);
        next
    }

    /// Restores the archived entry through protocol 23 automatic restoration,
    /// then extends its TTL. This mirrors a transaction carrying the entry in
    /// its restore list; Soroban SDK v28 has no contract-level `restore` API.
    pub fn restore(&self, extend_to: u32) {
        let key = self.key;
        let ledger = self.env.ledger();
        let protocol = ledger.get().protocol_version;
        ledger.set_protocol_version(23);
        self.env.as_contract(&self.contract, || {
            let _: Val = self.env.storage().persistent().get(&key).unwrap();
            self.env
                .storage()
                .persistent()
                .extend_ttl(&key, 0, extend_to);
        });
        ledger.set_protocol_version(protocol);
        self.archived.set(false);
    }

    /// Attempts a normal read. Before restoration, returns the host's
    /// `Storage/MissingValue` error for the archived entry.
    pub fn try_read(&self) -> Result<Val, soroban_sdk::Error> {
        if self.archived.get() {
            return Err(soroban_sdk::Error::from_type_and_code(
                soroban_sdk::xdr::ScErrorType::Storage,
                soroban_sdk::xdr::ScErrorCode::MissingValue,
            ));
        }
        let key = self.key;
        self.env
            .try_as_contract::<Val, soroban_sdk::Error>(&self.contract, || {
                self.env
                    .storage()
                    .persistent()
                    .get::<Val, Val>(&key)
                    .unwrap()
            })
            .map_err(|error| match error {
                Ok(error) => error,
                Err(invoke_error) => panic!("storage read invocation failed: {invoke_error:?}"),
            })
    }

    /// Reads the value after restoration.
    pub fn read(&self) -> Option<Val> {
        self.try_read().ok()
    }

    /// Returns the current TTL for the persistent entry.
    pub fn ttl(&self) -> u32 {
        let key = self.key;
        self.env.as_contract(&self.contract, || {
            self.env.storage().persistent().get_ttl(&key)
        })
    }

    /// Returns the original value retained by the simulator.
    pub fn original(&self) -> Val {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::StateEvictionSimulator;
    use soroban_sdk::{contract, Env, IntoVal, Symbol, TryFromVal};

    #[contract]
    struct Empty;

    #[test]
    fn persistent_value_archives_then_restores_with_original_contents() {
        let env = Env::default();
        let contract = env.register(Empty, ());
        let key = Symbol::new(&env, "eviction").into_val(&env);
        let value = 37u32.into_val(&env);
        let mut simulator = StateEvictionSimulator::new(&env, &contract, key, value);
        simulator.store();
        simulator.advance_past_ttl();

        let error = simulator.try_read().unwrap_err();
        assert!(error.is_type(soroban_sdk::xdr::ScErrorType::Storage));

        simulator.restore(20);
        let restored = simulator.read().unwrap();
        assert_eq!(u32::try_from_val(&env, &restored).unwrap(), 37);
        assert_eq!(u32::try_from_val(&env, &simulator.original()).unwrap(), 37);
        assert!(simulator.ttl() >= 20);
    }
}
