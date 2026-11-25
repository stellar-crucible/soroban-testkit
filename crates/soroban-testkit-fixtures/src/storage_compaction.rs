//! Deterministic storage expiration and archival for test environments.

use std::rc::Rc;

use soroban_env_host::{storage::AccessType, HostError};
use soroban_sdk::{
    xdr::{ContractDataDurability, LedgerEntryData, LedgerKey},
    Env,
};

/// Summary of storage entries removed or archived by a compaction pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StorageCompactionReport {
    /// Ledger sequence at which expiration was evaluated.
    pub ledger: u32,
    /// Contract data entries inspected.
    pub scanned_entries: usize,
    /// Expired temporary entries removed from mock host storage.
    pub pruned_temporary_entries: usize,
    /// Expired persistent entries removed to model their archived read state.
    pub archived_persistent_entries: usize,
}

/// Purges expired contract data from a Soroban test environment.
///
/// The compactor evaluates the host's recorded live-until ledger for every
/// contract data entry, rather than relying on a contract to read a key first.
pub struct StorageCompactor {
    env: Env,
}

impl StorageCompactor {
    /// Creates a compactor for `env`.
    pub fn new(env: &Env) -> Self {
        Self { env: env.clone() }
    }

    /// Removes contract data entries whose live-until ledger is at or before
    /// the current ledger sequence.
    ///
    /// Expired persistent data is removed from the mock host so reads fail as
    /// they do for archived state; expired temporary data reads as absent.
    ///
    /// # Errors
    ///
    /// Returns a host error if the mock storage cannot be updated.
    pub fn prune_expired(&self) -> Result<StorageCompactionReport, HostError> {
        let ledger = self.env.ledger().sequence();
        let snapshot = self.env.to_ledger_snapshot();
        let mut report = StorageCompactionReport {
            ledger,
            ..StorageCompactionReport::default()
        };
        let mut expired = Vec::new();

        for (key, (entry, live_until)) in &snapshot.ledger_entries {
            let LedgerKey::ContractData(_) = &**key else {
                continue;
            };
            let LedgerEntryData::ContractData(data) = &entry.data else {
                continue;
            };

            report.scanned_entries += 1;
            if live_until.is_some_and(|live_until| live_until <= ledger) {
                match data.durability {
                    ContractDataDurability::Temporary => {
                        report.pruned_temporary_entries += 1;
                    }
                    ContractDataDurability::Persistent => {
                        report.archived_persistent_entries += 1;
                    }
                }
                expired.push((**key).clone());
            }
        }

        for key in expired {
            self.env
                .host()
                .setup_storage_entry(Rc::new(key), None, AccessType::ReadWrite)?;
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::StorageCompactor;
    use crate::TestContext;
    use soroban_sdk::{
        contract, contractimpl,
        testutils::{storage::Temporary as _, Ledger as _},
        Env, Symbol,
    };

    #[contract]
    struct StorageFixture;

    #[contractimpl]
    impl StorageFixture {
        pub fn put(env: Env, key: Symbol, temporary: bool, value: u32, ttl: u32) {
            env.storage().instance().extend_ttl(100, 100);
            if temporary {
                env.storage().temporary().set(&key, &value);
                if ttl > 0 {
                    env.storage().temporary().extend_ttl(&key, ttl, ttl);
                }
            } else {
                env.storage().persistent().set(&key, &value);
                if ttl > 0 {
                    env.storage().persistent().extend_ttl(&key, ttl, ttl);
                }
            }
        }

        pub fn get_temporary(env: Env, key: Symbol) -> Option<u32> {
            env.storage().temporary().get(&key)
        }

        pub fn get_persistent(env: Env, key: Symbol) -> Option<u32> {
            env.storage().persistent().get(&key)
        }

        pub fn get_temporary_ttl(env: Env, key: Symbol) -> u32 {
            env.storage().temporary().get_ttl(&key)
        }
    }

    fn setup() -> (TestContext, soroban_sdk::Address) {
        let ctx = TestContext::new();
        ctx.env.ledger().set_min_temp_entry_ttl(1);
        ctx.env.ledger().set_min_persistent_entry_ttl(1);
        ctx.env.ledger().set_max_entry_ttl(100);
        ctx.env.ledger().set_protocol_version(22);
        let contract = ctx.env.register(StorageFixture, ());
        (ctx, contract)
    }

    #[test]
    fn compaction_prunes_expired_entries_and_preserves_live_storage() {
        let (mut ctx, contract) = setup();
        let client = StorageFixtureClient::new(&ctx.env, &contract);
        let expired_temporary = Symbol::new(&ctx.env, "expired_temp");
        let live_temporary = Symbol::new(&ctx.env, "live_temp");
        let expired_persistent = Symbol::new(&ctx.env, "expired_persist");
        let live_persistent = Symbol::new(&ctx.env, "live_persist");
        client.put(&expired_temporary, &true, &1, &0);
        client.put(&live_temporary, &true, &2, &20);
        client.put(&expired_persistent, &false, &3, &0);
        client.put(&live_persistent, &false, &4, &20);

        let current_ledger = ctx.advance_ledger(1);
        let report = ctx.prune_expired_storage().unwrap();

        assert_eq!(report.ledger, current_ledger);
        assert_eq!(report.pruned_temporary_entries, 1);
        assert_eq!(report.archived_persistent_entries, 1);
        assert_eq!(client.get_temporary(&expired_temporary), None);
        assert_eq!(client.get_temporary(&live_temporary), Some(2));
        assert_eq!(
            client
                .try_get_persistent(&expired_persistent)
                .unwrap()
                .unwrap(),
            None,
            "an archived persistent entry should no longer be readable"
        );
        assert_eq!(client.get_persistent(&live_persistent), Some(4));
    }

    #[test]
    fn compactor_handles_empty_and_repeated_passes() {
        let (ctx, contract) = setup();
        let client = StorageFixtureClient::new(&ctx.env, &contract);
        let key = Symbol::new(&ctx.env, "live");
        client.put(&key, &false, &42, &20);

        let first = ctx.prune_expired_storage().unwrap();
        let second = ctx.prune_expired_storage().unwrap();

        assert!(first.scanned_entries > 0);
        assert_eq!(first.archived_persistent_entries, 0);
        assert_eq!(second.archived_persistent_entries, 0);
        assert_eq!(client.get_persistent(&key), Some(42));
    }

    #[test]
    fn expires_at_the_current_ledger_inclusive() {
        let (mut ctx, contract) = setup();
        let client = StorageFixtureClient::new(&ctx.env, &contract);
        let key = Symbol::new(&ctx.env, "expires");
        client.put(&key, &true, &1, &20);
        let ttl = client.get_temporary_ttl(&key);

        ctx.advance_ledger(ttl);
        assert_eq!(
            ctx.prune_expired_storage()
                .unwrap()
                .pruned_temporary_entries,
            1
        );
        assert_eq!(client.get_temporary(&key), None);
    }

    #[test]
    fn standalone_compactor_uses_the_environment_ledger() {
        let (ctx, _) = setup();
        let report = StorageCompactor::new(&ctx.env).prune_expired().unwrap();
        assert_eq!(report.ledger, ctx.sequence());
    }
}
