use std::rc::Rc;

use soroban_env_host::storage::AccessType;
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::xdr::{ContractDataDurability, LedgerEntry, LedgerKey, ScAddress, ScVal};
use soroban_sdk::Env;

/// One persistent entry a sweep archived, kept whole so it can be restored.
///
/// The `LedgerEntry` is the one the host held before the sweep, not a
/// re-encoding of it, so restoring an entry gives back exactly the data the
/// contract wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedEntry {
    key: LedgerKey,
    entry: LedgerEntry,
    live_until: Option<u32>,
}

impl ArchivedEntry {
    /// The ledger key the entry lives under.
    pub fn key(&self) -> &LedgerKey {
        &self.key
    }

    /// The entry itself: the data the contract wrote, and its metadata.
    pub fn entry(&self) -> &LedgerEntry {
        &self.entry
    }

    /// The last ledger the entry was live at, before the sweep archived it.
    pub fn live_until(&self) -> Option<u32> {
        self.live_until
    }

    /// The contract that owned the entry, if the entry is contract data.
    pub fn contract(&self) -> Option<&ScAddress> {
        match &self.key {
            LedgerKey::ContractData(data) => Some(&data.contract),
            _ => None,
        }
    }
}

/// What one archival sweep did to the ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchivalSweep {
    /// The ledger sequence the sweep ran at.
    pub ledger: u32,
    /// Persistent entries moved into the archive, restorable by the simulator.
    pub archived: usize,
    /// Temporary entries whose TTL had elapsed, dropped for good.
    pub evicted_temporary: usize,
    /// Contract-data entries still inside their TTL after the sweep, including
    /// each contract's instance entry.
    pub kept_live: usize,
    /// The keys of the temporary entries the sweep evicted.
    pub evicted_keys: Vec<LedgerKey>,
}

impl ArchivalSweep {
    /// Entries the sweep took off the live ledger, archived plus evicted.
    pub fn swept(&self) -> usize {
        self.archived + self.evicted_temporary
    }
}

/// Simulates the network's archival sweep over a test environment.
///
/// A real ledger archives a persistent entry whose rent has run out: the entry
/// is no longer readable until a restore operation puts it back. A temporary
/// entry that expires is gone for good. The SDK test environment does not run
/// that sweep — it auto-restores a persistent entry the moment a test reads it
/// — so this simulator performs the sweep explicitly:
///
/// ```rust
/// use soroban_testkit_fixtures::archival::ArchivalSweepSimulator;
///
/// # fn sweep(env: &soroban_sdk::Env) {
/// let mut simulator = ArchivalSweepSimulator::new();
/// let sweep = simulator.sweep(env, 5_000); // advance 5,000 ledgers, then sweep
/// println!("archived {} entries at ledger {}", sweep.archived, sweep.ledger);
///
/// // Reads of the archived entries now find nothing until a restore runs.
/// let restored = simulator.restore_all(env, 5_000);
/// println!("restored {restored} entries");
/// # }
/// # sweep(&soroban_sdk::Env::default());
/// ```
///
/// The sweep covers every contract registered in the environment, keeps the
/// contract instance and code entries live so the contracts remain callable,
/// and records each archived entry with the exact `LedgerEntry` it had.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArchivalSweepSimulator {
    archived: Vec<ArchivedEntry>,
}

impl ArchivalSweepSimulator {
    /// A simulator holding nothing yet; every entry it archives is one a sweep
    /// of this instance took.
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance the ledger by `ledgers_ahead` and sweep every entry whose TTL
    /// has elapsed.
    ///
    /// Advancing is how a test crosses a TTL without hard-coding the network's
    /// minimum rent: write the entry, advance past its `live_until`, and the
    /// sweep takes it. Use [`ArchivalSweepSimulator::sweep_expired`] to sweep
    /// at the current sequence instead.
    pub fn sweep(&mut self, env: &Env, ledgers_ahead: u32) -> ArchivalSweep {
        let next = env.ledger().sequence().saturating_add(ledgers_ahead);
        env.ledger().set_sequence_number(next);
        self.sweep_expired(env)
    }

    /// Sweep every entry whose TTL has already elapsed, without moving the
    /// ledger first.
    ///
    /// Persistent contract-data entries are archived and kept for
    /// [`ArchivalSweepSimulator::restore_all`]. Temporary entries cannot be
    /// restored on a real network and are dropped here too. Contract instance
    /// and code entries are left live: archiving them would make the contract
    /// itself unreachable, and then no test could observe what the sweep did to
    /// its data.
    pub fn sweep_expired(&mut self, env: &Env) -> ArchivalSweep {
        let ledger = env.ledger().sequence();
        let stored = env
            .host()
            .get_stored_entries()
            .unwrap_or_else(|error| panic!("archival sweep could not read the ledger: {error:?}"));

        let mut sweep = ArchivalSweep {
            ledger,
            ..ArchivalSweep::default()
        };

        for (key, stored) in stored {
            let Some((entry, live_until)) = stored else {
                continue;
            };
            let LedgerKey::ContractData(data) = key.as_ref() else {
                continue;
            };
            let expired = matches!(live_until, Some(until) if until < ledger);

            if !expired {
                sweep.kept_live += 1;
                continue;
            }

            match data.durability {
                ContractDataDurability::Temporary => {
                    remove_entry(env, &key);
                    sweep.evicted_temporary += 1;
                    sweep.evicted_keys.push(key.as_ref().clone());
                }
                ContractDataDurability::Persistent => {
                    if data.key == ScVal::LedgerKeyContractInstance {
                        // The contract has to stay callable for the test to
                        // read its now-archived data entries.
                        sweep.kept_live += 1;
                        continue;
                    }
                    remove_entry(env, &key);
                    self.archived.push(ArchivedEntry {
                        key: key.as_ref().clone(),
                        entry: entry.as_ref().clone(),
                        live_until,
                    });
                    sweep.archived += 1;
                }
            }
        }

        sweep
    }

    /// Every entry this simulator has archived and can still restore.
    pub fn archived(&self) -> &[ArchivedEntry] {
        &self.archived
    }

    /// How many entries the archive holds.
    pub fn len(&self) -> usize {
        self.archived.len()
    }

    /// Whether the archive is empty — either because no sweep took anything, or
    /// because every entry has been restored.
    pub fn is_empty(&self) -> bool {
        self.archived.is_empty()
    }

    /// Whether `key` is in the archive.
    pub fn is_archived(&self, key: &LedgerKey) -> bool {
        self.archived.iter().any(|archived| &archived.key == key)
    }

    /// The archived record for `key`, if the sweep took it.
    pub fn entry(&self, key: &LedgerKey) -> Option<&ArchivedEntry> {
        self.archived.iter().find(|archived| &archived.key == key)
    }

    /// Put every archived entry back with `extra_ttl` ledgers of live time,
    /// returning how many were restored.
    ///
    /// This is the restore operation the protocol performs: each entry is live
    /// again with the data it had, and the archive no longer holds it. A
    /// restore always grants at least one ledger of life, so an entry restored
    /// with `0` is not immediately eligible for another sweep.
    pub fn restore_all(&mut self, env: &Env, extra_ttl: u32) -> usize {
        let archived = std::mem::take(&mut self.archived);
        for entry in &archived {
            restore_entry(env, entry, extra_ttl);
        }
        archived.len()
    }

    /// Put one archived entry back, if the sweep took it.
    ///
    /// Returns `false` when `key` is not in the archive — restoring twice is a
    /// no-op rather than a panic, which lets a test restore exactly the entries
    /// it cares about and leave the rest archived.
    pub fn restore(&mut self, env: &Env, key: &LedgerKey, extra_ttl: u32) -> bool {
        let Some(index) = self.archived.iter().position(|entry| &entry.key == key) else {
            return false;
        };
        let entry = self.archived.remove(index);
        restore_entry(env, &entry, extra_ttl);
        true
    }

    /// Fail unless `key` was archived and the host no longer holds a live entry
    /// for it.
    ///
    /// # Panics
    ///
    /// Panics when `key` is not in this simulator's archive, or when the host
    /// still holds a live entry for it.
    pub fn assert_archived(&self, env: &Env, key: &LedgerKey) {
        assert!(
            self.is_archived(key),
            "Expected {key:?} to be archived, but the sweep did not take it (the archive holds {} entries)",
            self.archived.len()
        );
        assert!(
            live_entry(env, key).is_none(),
            "Expected {key:?} to be absent from the ledger after the sweep, but the host still holds a live entry for it"
        );
    }

    /// Fail unless `key` left the archive and the host holds a live entry for
    /// it again.
    ///
    /// # Panics
    ///
    /// Panics when `key` is still in the archive, or when the host has no live
    /// entry for it.
    pub fn assert_restored(&self, env: &Env, key: &LedgerKey) {
        assert!(
            !self.is_archived(key),
            "Expected {key:?} to have been restored, but it is still in the archive"
        );
        assert!(
            live_entry(env, key).is_some(),
            "Expected {key:?} to be live after the restore, but the host holds no entry for it"
        );
    }
}

fn remove_entry(env: &Env, key: &Rc<LedgerKey>) {
    env.host()
        .setup_storage_entry(key.clone(), None, AccessType::ReadOnly)
        .unwrap_or_else(|error| {
            panic!(
                "archival sweep could not remove {:?} from the ledger: {error:?}",
                key.as_ref()
            )
        });
}

fn restore_entry(env: &Env, archived: &ArchivedEntry, extra_ttl: u32) {
    let live_until = env.ledger().sequence().saturating_add(extra_ttl.max(1));
    env.host()
        .add_ledger_entry(
            &Rc::new(archived.key.clone()),
            &Rc::new(archived.entry.clone()),
            Some(live_until),
        )
        .unwrap_or_else(|error| {
            panic!(
                "restore operation could not put {:?} back on the ledger: {error:?}",
                archived.key
            )
        });
}

fn live_entry(env: &Env, key: &LedgerKey) -> Option<(Rc<LedgerEntry>, Option<u32>)> {
    env.host()
        .get_ledger_entry(&Rc::new(key.clone()))
        .unwrap_or_else(|error| panic!("could not read the ledger entry {key:?}: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::ArchivalSweepSimulator;
    use crate::builder::TestContextBuilder;
    use soroban_sdk::xdr::{LedgerKey, ScAddress};
    use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};
    use soroban_testkit_core::storage::StorageSnapshot;

    #[contract]
    pub struct Registry;

    #[contractimpl]
    impl Registry {
        pub fn set(env: Env, balance: u32) {
            env.storage()
                .persistent()
                .set(&symbol_short!("bal"), &balance);
        }

        pub fn get(env: Env) -> Option<u32> {
            env.storage().persistent().get(&symbol_short!("bal"))
        }

        /// The restore operation a contract runs on an entry it expects to
        /// still be live.
        pub fn bump(env: Env, ttl: u32) {
            env.storage()
                .persistent()
                .extend_ttl(&symbol_short!("bal"), ttl, ttl);
        }

        pub fn set_temporary(env: Env, value: u32) {
            env.storage()
                .temporary()
                .set(&symbol_short!("nonce"), &value);
        }

        pub fn get_temporary(env: Env) -> Option<u32> {
            env.storage().temporary().get(&symbol_short!("nonce"))
        }
    }

    fn registry(env: &Env) -> (Address, RegistryClient<'_>) {
        let id = env.register(Registry, ());
        let client = RegistryClient::new(env, &id);
        (id, client)
    }

    fn live_until_of(env: &Env, contract: &Address, key: &str) -> u32 {
        StorageSnapshot::capture(env, contract)
            .get(key)
            .unwrap_or_else(|| panic!("the contract holds no {key:?} entry"))
            .live_until
            .expect("a contract-data entry expires")
    }

    /// How many ledgers it takes to leave `key` outside its TTL.
    ///
    /// Read back from the ledger instead of assuming the network's minimum
    /// rent, so a change to that constant does not turn these tests into
    /// arithmetic about it.
    fn ledgers_until_expiry(env: &Env, contract: &Address, key: &str) -> u32 {
        live_until_of(env, contract, key)
            .saturating_sub(env.ledger().sequence())
            .saturating_add(1)
    }

    #[test]
    fn a_sweep_archives_a_persistent_entry_past_its_ttl() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&100);
        let live_until = live_until_of(&env, &contract, "bal");

        let mut simulator = ArchivalSweepSimulator::new();
        let sweep = simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));

        assert_eq!(sweep.ledger, live_until + 1);
        assert_eq!(sweep.archived, 1, "the entry was past its TTL");
        assert_eq!(sweep.swept(), 1);
        assert_eq!(sweep.evicted_temporary, 0);
        assert_eq!(simulator.len(), 1);

        let entry = &simulator.archived()[0];
        assert_eq!(entry.live_until(), Some(live_until));
        assert_eq!(entry.contract(), Some(&ScAddress::from(&contract)));
        simulator.assert_archived(&env, entry.key());
        assert_eq!(
            client.get(),
            None,
            "the contract can no longer read the archived entry"
        );
    }

    #[test]
    fn a_sweep_at_the_current_ledger_leaves_a_fresh_entry_alone() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (_contract, client) = registry(&env);
        client.set(&100);

        let mut simulator = ArchivalSweepSimulator::new();
        let sweep = simulator.sweep_expired(&env);

        assert_eq!(sweep.ledger, env.ledger().sequence());
        assert_eq!(sweep.swept(), 0);
        assert_eq!(
            sweep.kept_live, 2,
            "the data entry and the contract instance are both live"
        );
        assert!(simulator.is_empty());
        assert_eq!(client.get(), Some(100));
    }

    #[test]
    fn the_sweep_covers_every_registered_contract() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (first, first_client) = registry(&env);
        let (_second, second_client) = registry(&env);
        first_client.set(&1);
        second_client.set(&2);

        let mut simulator = ArchivalSweepSimulator::new();
        let sweep = simulator.sweep(&env, ledgers_until_expiry(&env, &first, "bal"));

        assert_eq!(sweep.archived, 2, "both contracts' entries were swept");
        assert_eq!(first_client.get(), None);
        assert_eq!(second_client.get(), None);

        let contracts: Vec<&ScAddress> = simulator
            .archived()
            .iter()
            .filter_map(|entry| entry.contract())
            .collect();
        assert_eq!(contracts.len(), 2);
        assert_ne!(contracts[0], contracts[1]);
    }

    #[test]
    fn restore_brings_back_the_original_data() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&4_242);
        let before = StorageSnapshot::capture(&env, &contract);
        let value_before = before.get("bal").expect("the entry exists").value.clone();

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));
        let key = simulator.archived()[0].key().clone();
        assert_eq!(client.get(), None);

        assert_eq!(simulator.restore_all(&env, 1_000), 1);
        assert!(simulator.is_empty(), "a restored entry leaves the archive");
        simulator.assert_restored(&env, &key);

        assert_eq!(client.get(), Some(4_242), "the value came back unchanged");
        let after = StorageSnapshot::capture(&env, &contract);
        assert_eq!(
            after.get("bal").expect("the entry is live again").value,
            value_before,
            "the restored entry renders the same value it had before the sweep"
        );
        assert_eq!(
            after.get("bal").unwrap().live_until,
            Some(env.ledger().sequence() + 1_000),
            "the restore granted the rent it was asked for"
        );
    }

    #[test]
    fn restoring_one_entry_leaves_the_other_archived() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (first, first_client) = registry(&env);
        let (_second, second_client) = registry(&env);
        first_client.set(&1);
        second_client.set(&2);

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &first, "bal"));
        assert_eq!(simulator.len(), 2);

        let wanted = ScAddress::from(&first);
        let key = simulator
            .archived()
            .iter()
            .find(|entry| entry.contract() == Some(&wanted))
            .map(|entry| entry.key().clone())
            .expect("the sweep archived the first contract's entry");
        assert!(simulator.entry(&key).is_some());

        assert!(simulator.restore(&env, &key, 100));
        assert_eq!(simulator.len(), 1, "the other entry is still archived");
        assert!(!simulator.is_archived(&key));
        assert_eq!(first_client.get(), Some(1));
        assert_eq!(second_client.get(), None);
    }

    #[test]
    fn restoring_twice_is_a_no_op() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&1);

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));
        let key = simulator.archived()[0].key().clone();

        assert!(simulator.restore(&env, &key, 100));
        assert!(!simulator.restore(&env, &key, 100));
        assert_eq!(client.get(), Some(1));
    }

    #[test]
    fn a_restore_grants_at_least_one_ledger_of_life() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&1);

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));
        simulator.restore_all(&env, 0);

        let ledger = env.ledger().sequence();
        assert_eq!(
            StorageSnapshot::capture(&env, &contract)
                .get("bal")
                .unwrap()
                .live_until,
            Some(ledger + 1)
        );
        let sweep = simulator.sweep_expired(&env);
        assert_eq!(sweep.swept(), 0, "the entry is still live at this ledger");
        assert_eq!(client.get(), Some(1));
    }

    #[test]
    fn a_read_finds_nothing_and_a_restore_operation_fails() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&7);

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));

        assert_eq!(client.get(), None, "the archived entry reads as missing");
        assert!(
            StorageSnapshot::capture(&env, &contract)
                .get("bal")
                .is_none(),
            "the archived entry is gone from the contract's storage"
        );

        // The host raises `Storage/MissingValue` — StorageNotFound — for an
        // operation that requires the entry. A native test contract is a guest
        // call, and the env narrows host codes at that boundary so a contract
        // cannot observe them, so what the test sees is the narrowed code.
        let failure = client
            .try_bump(&100)
            .expect_err("extending the TTL of an archived entry cannot succeed");
        let reason = format!("{failure:?}");
        assert!(
            reason.contains("Error(Context, InvalidAction)"),
            "expected the missing-entry failure, got {reason}"
        );
    }

    #[test]
    fn a_sweep_evicts_an_expired_temporary_entry_for_good() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (_contract, client) = registry(&env);
        client.set_temporary(&9);

        let mut simulator = ArchivalSweepSimulator::new();
        let sweep = simulator.sweep(&env, 100);

        assert_eq!(sweep.evicted_temporary, 1);
        assert_eq!(sweep.evicted_keys.len(), 1);
        assert_eq!(sweep.archived, 0, "a temporary entry is not restorable");
        assert_eq!(client.get_temporary(), None);

        assert_eq!(simulator.restore_all(&env, 100), 0);
        assert_eq!(client.get_temporary(), None, "no restore brings it back");
    }

    #[test]
    fn the_contract_stays_callable_after_the_sweep() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&1);

        let mut simulator = ArchivalSweepSimulator::new();
        // Far past every TTL in the environment, including the instance's,
        // which the sweep deliberately leaves alone.
        simulator.sweep(&env, 100_000);

        assert_eq!(simulator.len(), 1, "only the data entry was archived");
        assert_eq!(
            StorageSnapshot::capture(&env, &contract)
                .in_tier(soroban_testkit_core::storage::StorageTier::Instance)
                .len(),
            0,
            "the registry keeps nothing in instance storage"
        );
        assert_eq!(
            client.get(),
            None,
            "the call reached the contract instead of failing on a missing instance"
        );
    }

    #[test]
    fn several_sweeps_do_not_duplicate_an_archived_entry() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&1);

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));
        let sweep = simulator.sweep(&env, 10_000);

        assert_eq!(sweep.archived, 0, "the entry left the ledger already");
        assert_eq!(simulator.len(), 1);
        assert_eq!(simulator.restore_all(&env, 100), 1);
        assert_eq!(client.get(), Some(1));
    }

    #[test]
    fn a_context_can_archive_its_own_ledger() {
        let mut ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&1);
        let ledgers = ledgers_until_expiry(&env, &contract, "bal");

        let mut simulator = ctx.archive_expired(ledgers);

        assert_eq!(simulator.len(), 1);
        assert_eq!(client.get(), None);

        let key = simulator.archived()[0].key().clone();
        simulator.restore_all(&ctx.env, 100);
        simulator.assert_restored(&ctx.env, &key);
        assert_eq!(client.get(), Some(1));
    }

    #[test]
    fn an_archived_entry_exposes_its_key_and_xdr_value() {
        let ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (contract, client) = registry(&env);
        client.set(&31);

        let mut simulator = ArchivalSweepSimulator::new();
        simulator.sweep(&env, ledgers_until_expiry(&env, &contract, "bal"));

        let entry = &simulator.archived()[0];
        assert!(matches!(entry.key(), LedgerKey::ContractData(_)));
        match entry.entry().data {
            soroban_sdk::xdr::LedgerEntryData::ContractData(ref data) => {
                assert_eq!(data.val, soroban_sdk::xdr::ScVal::U32(31));
            }
            ref other => panic!("expected contract data, got {other:?}"),
        }
    }
}
