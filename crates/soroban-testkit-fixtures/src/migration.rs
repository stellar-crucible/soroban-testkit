//! Drive a contract's schema migrations from a test and check each step.
//!
//! A migration is a contract call that rewrites stored data from one schema
//! to the next and records the new schema version in instance storage.
//! [`MigrationRunner`] reads that version before and after every step, so a
//! test can check the four things a migration gets wrong:
//!
//! - **Monotonic versions.** Each step must go from `vN` to `vN+1`; skipping a
//!   version or going backwards is refused before the contract is called.
//! - **Exactly once.** Re-running a migration that already ran is refused with
//!   a diagnostic naming when it ran, instead of silently double-applying it.
//! - **Recorded versions.** A step that does not leave the target version in
//!   instance storage fails, because the next deploy would run it again.
//! - **Atomicity.** A step that fails must leave storage exactly as it found
//!   it; a failure that leaves half-migrated state is reported as such.
//!
//! Each successful step returns a [`MigrationRecord`] carrying the storage
//! snapshots on either side of it, for before-and-after schema assertions.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use soroban_sdk::{symbol_short, Address, Env, IntoVal, TryFromVal, Val};
use soroban_testkit_core::storage::{StorageDiff, StorageSnapshot};

/// Why a migration step was refused or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MigrationError {
    /// The target version already ran through this runner.
    AlreadyApplied { to: u32, current: u32, step: usize },
    /// The target version is at or below the contract's current version.
    NotForward { from: u32, to: u32 },
    /// The target version skips one or more intermediate versions.
    SkipsVersion { from: u32, to: u32 },
    /// The step finished, but instance storage does not hold the target version.
    VersionNotRecorded { expected: u32, found: Option<u32> },
    /// The step panicked and storage was left untouched.
    Failed { from: u32, to: u32, message: String },
    /// The step panicked *and* left storage changed: a non-atomic migration.
    NotAtomic {
        from: u32,
        to: u32,
        message: String,
        leftover: String,
    },
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MigrationError::AlreadyApplied { to, current, step } => write!(
                f,
                "migration to v{to} was already executed (step {step} of this runner) and the contract is at v{current}; running it again would double-apply it"
            ),
            MigrationError::NotForward { from, to } => write!(
                f,
                "migration to v{to} does not move forward: the contract is already at v{from}"
            ),
            MigrationError::SkipsVersion { from, to } => write!(
                f,
                "migration from v{from} to v{to} skips v{}; migrations must advance one version at a time",
                from + 1
            ),
            MigrationError::VersionNotRecorded { expected, found } => match found {
                Some(found) => write!(
                    f,
                    "migration to v{expected} finished but instance storage records v{found}; the step must write its own version"
                ),
                None => write!(
                    f,
                    "migration to v{expected} finished but instance storage holds no version key; the step must write its own version"
                ),
            },
            MigrationError::Failed { from, to, message } => write!(
                f,
                "migration from v{from} to v{to} failed and storage was left unchanged: {message}"
            ),
            MigrationError::NotAtomic {
                from,
                to,
                message,
                leftover,
            } => write!(
                f,
                "migration from v{from} to v{to} failed but left storage partially migrated ({leftover}): {message}"
            ),
        }
    }
}

impl std::error::Error for MigrationError {}

/// One completed migration step.
#[derive(Clone, Debug)]
pub struct MigrationRecord {
    pub from: u32,
    pub to: u32,
    /// The contract's storage just before the step ran.
    pub before: StorageSnapshot,
    /// The contract's storage just after it finished.
    pub after: StorageSnapshot,
}

impl MigrationRecord {
    /// What the step added, removed and rewrote.
    pub fn diff(&self) -> StorageDiff {
        self.before.diff(&self.after)
    }

    /// Fail unless `key` was present before the step and gone after it.
    pub fn assert_removed(&self, key: &str) {
        self.before.assert_entry_removed(&self.after, key);
    }

    /// Fail unless `key` was absent before the step and present after it.
    pub fn assert_added(&self, key: &str) {
        self.before.assert_entry_added(&self.after, key);
    }

    /// Fail unless `from_key` moved to `to_key`: present only before and only
    /// after respectively.
    pub fn assert_renamed(&self, from_key: &str, to_key: &str) {
        self.assert_removed(from_key);
        self.assert_added(to_key);
    }

    /// Fail unless the storage keys before the step are exactly `expected`.
    pub fn assert_schema_before(&self, expected: &[&str]) {
        assert_keys(&self.before, expected, &format!("before v{}", self.to));
    }

    /// Fail unless the storage keys after the step are exactly `expected`.
    pub fn assert_schema_after(&self, expected: &[&str]) {
        assert_keys(&self.after, expected, &format!("after v{}", self.to));
    }
}

/// Runs a contract's schema migrations one version at a time.
///
/// `C` is whatever the migration closure receives — the contract client when
/// built with [`MigrationRunner::with_client`], or the contract's [`Address`]
/// when built with [`MigrationRunner::new`].
///
/// ```rust,ignore
/// let mut runner = MigrationRunner::with_client(&ctx.env, &id, VaultClient::new(&ctx.env, &id));
/// assert_eq!(runner.current_version(), 1);
///
/// runner.execute_migration(2, |client| client.migrate_v1_to_v2());
/// assert_eq!(runner.current_version(), 2);
/// ```
pub struct MigrationRunner<'a, C = Address> {
    env: &'a Env,
    contract: Address,
    client: C,
    version_key: Val,
    unversioned: u32,
    history: Vec<MigrationRecord>,
}

impl<'a> MigrationRunner<'a, Address> {
    /// A runner whose migration closures receive the contract's address.
    pub fn new(env: &'a Env, contract: &Address) -> Self {
        Self::with_client(env, contract, contract.clone())
    }
}

impl<'a, C> MigrationRunner<'a, C> {
    /// A runner whose migration closures receive `client`.
    pub fn with_client(env: &'a Env, contract: &Address, client: C) -> Self {
        Self {
            env,
            contract: contract.clone(),
            client,
            version_key: symbol_short!("version").into_val(env),
            unversioned: 1,
            history: Vec::new(),
        }
    }

    /// Read the schema version from a different instance-storage key.
    ///
    /// Defaults to the symbol `version`. Pass whatever the contract uses —
    /// `symbol_short!("VER")`, or a `#[contracttype]` enum variant such as
    /// `DataKey::Version`.
    pub fn with_version_key(mut self, key: impl IntoVal<Env, Val>) -> Self {
        self.version_key = key.into_val(self.env);
        self
    }

    /// The version to assume when instance storage holds no version key.
    ///
    /// Defaults to `1`: a contract that predates versioning is treated as v1,
    /// which is how most first deploys look.
    pub fn assuming_unversioned_is(mut self, version: u32) -> Self {
        self.unversioned = version;
        self
    }

    /// The contract's address.
    pub fn contract(&self) -> &Address {
        &self.contract
    }

    /// The client migration closures receive.
    pub fn client(&self) -> &C {
        &self.client
    }

    /// The version stored in instance storage, if any.
    pub fn stored_version(&self) -> Option<u32> {
        let key = self.version_key;
        let env = self.env;
        env.as_contract(&self.contract, || {
            let storage = env.storage().instance();
            storage.get::<Val, Val>(&key).map(|raw| {
                u32::try_from_val(env, &raw).unwrap_or_else(|_| {
                    panic!("the version key in instance storage does not hold a u32")
                })
            })
        })
    }

    /// The contract's schema version: the stored one, or the unversioned
    /// default when nothing is stored.
    pub fn current_version(&self) -> u32 {
        self.stored_version().unwrap_or(self.unversioned)
    }

    /// Every step this runner completed, oldest first.
    pub fn history(&self) -> &[MigrationRecord] {
        &self.history
    }

    /// Storage as it stands now.
    pub fn snapshot(&self) -> StorageSnapshot {
        StorageSnapshot::capture(self.env, &self.contract)
    }

    /// Fail unless the contract's storage keys are exactly `expected`.
    pub fn assert_schema(&self, expected: &[&str]) {
        assert_keys(
            &self.snapshot(),
            expected,
            &format!("at v{}", self.current_version()),
        );
    }

    /// Run one migration step to version `to`, panicking with a diagnostic if
    /// it is refused or fails.
    pub fn execute_migration<R>(
        &mut self,
        to: u32,
        migrate: impl FnOnce(&C) -> R,
    ) -> &MigrationRecord {
        if let Err(error) = self.try_execute_migration(to, migrate) {
            panic!("{error}");
        }
        self.history.last().expect("a step was just recorded")
    }

    /// Run one migration step to version `to`.
    ///
    /// The step is refused without calling the contract when `to` is not
    /// exactly one past the current version or already ran through this
    /// runner. A step that panics is caught and its storage compared with the
    /// snapshot taken before it, so the error says whether it rolled back.
    pub fn try_execute_migration<R>(
        &mut self,
        to: u32,
        migrate: impl FnOnce(&C) -> R,
    ) -> Result<&MigrationRecord, MigrationError> {
        let from = self.current_version();

        if let Some(step) = self.history.iter().position(|record| record.to == to) {
            return Err(MigrationError::AlreadyApplied {
                to,
                current: from,
                step: step + 1,
            });
        }
        if to <= from {
            return Err(MigrationError::NotForward { from, to });
        }
        if to != from + 1 {
            return Err(MigrationError::SkipsVersion { from, to });
        }

        let before = self.snapshot();
        let client = &self.client;
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| migrate(client)));
        let after = self.snapshot();

        if let Err(payload) = outcome {
            let message = panic_message(payload.as_ref());
            let diff = before.diff(&after);
            return Err(if diff.is_empty() {
                MigrationError::Failed { from, to, message }
            } else {
                MigrationError::NotAtomic {
                    from,
                    to,
                    message,
                    leftover: describe(&diff),
                }
            });
        }

        let found = self.stored_version();
        if found != Some(to) {
            return Err(MigrationError::VersionNotRecorded {
                expected: to,
                found,
            });
        }

        self.history.push(MigrationRecord {
            from,
            to,
            before,
            after,
        });
        Ok(self.history.last().expect("a step was just recorded"))
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "the step panicked with a non-string payload".to_string()
    }
}

fn describe(diff: &StorageDiff) -> String {
    let mut parts = Vec::new();
    parts.extend(diff.added().iter().map(|e| format!("+ {}", e.qualified())));
    parts.extend(
        diff.removed()
            .iter()
            .map(|e| format!("- {}", e.qualified())),
    );
    parts.extend(
        diff.modified()
            .iter()
            .map(|c| format!("~ {}:{}", c.tier.as_str(), c.key)),
    );
    parts.join(", ")
}

fn assert_keys(snapshot: &StorageSnapshot, expected: &[&str], when: &str) {
    let mut actual: Vec<&str> = snapshot.entries().iter().map(|e| e.key.as_str()).collect();
    actual.sort_unstable();
    let mut wanted = expected.to_vec();
    wanted.sort_unstable();
    assert_eq!(
        actual, wanted,
        "storage schema {when} does not match: found keys {actual:?}, expected {wanted:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::{MigrationError, MigrationRunner};
    use crate::TestContext;
    use soroban_sdk::{
        contract, contractimpl, contracttype, symbol_short, Address, Env, Map, Symbol,
    };

    /// A vault whose schema went through three versions:
    ///
    /// - v1: one persistent `balance` per depositor, no version key at all.
    /// - v2: balances folded into one persistent `ledger` map, version 2.
    /// - v3: an instance `fee_bps` setting added, version 3.
    #[contract]
    pub struct Vault;

    const VERSION: Symbol = symbol_short!("version");
    const BALANCE: Symbol = symbol_short!("balance");
    const LEDGER: Symbol = symbol_short!("ledger");
    const FEE: Symbol = symbol_short!("fee_bps");

    #[contractimpl]
    impl Vault {
        pub fn deposit_v1(env: Env, who: Address, amount: i128) {
            env.storage().persistent().set(&BALANCE, &(who, amount));
        }

        pub fn migrate_v1_to_v2(env: Env) {
            let (who, amount): (Address, i128) = env.storage().persistent().get(&BALANCE).unwrap();
            let mut ledger: Map<Address, i128> = Map::new(&env);
            ledger.set(who, amount);
            env.storage().persistent().set(&LEDGER, &ledger);
            env.storage().persistent().remove(&BALANCE);
            env.storage().instance().set(&VERSION, &2u32);
        }

        pub fn migrate_v2_to_v3(env: Env) {
            env.storage().instance().set(&FEE, &30u32);
            env.storage().instance().set(&VERSION, &3u32);
        }

        /// Writes the new layout, then fails before finishing.
        pub fn migrate_v1_to_v2_broken(env: Env) {
            env.storage()
                .persistent()
                .set(&LEDGER, &Map::<Address, i128>::new(&env));
            panic!("ran out of balances to move");
        }

        /// Migrates the data but forgets to record the version.
        pub fn migrate_v1_to_v2_unversioned(env: Env) {
            env.storage().persistent().remove(&BALANCE);
        }

        pub fn balance_v2(env: Env, who: Address) -> i128 {
            let ledger: Map<Address, i128> = env.storage().persistent().get(&LEDGER).unwrap();
            ledger.get(who).unwrap_or(0)
        }
    }

    fn v1_vault() -> (TestContext, Address, Address) {
        let mut ctx = TestContext::new();
        let depositor = ctx.add_user().clone();
        let id = ctx.env.register(Vault, ());
        VaultClient::new(&ctx.env, &id).deposit_v1(&depositor, &1_000);
        (ctx, id, depositor)
    }

    #[test]
    fn an_unversioned_contract_starts_at_v1() {
        let (ctx, id, _) = v1_vault();
        let runner = MigrationRunner::new(&ctx.env, &id);

        assert_eq!(runner.stored_version(), None);
        assert_eq!(runner.current_version(), 1);
        runner.assert_schema(&["balance"]);
    }

    #[test]
    fn a_v1_to_v2_to_v3_pipeline_upgrades_state_cleanly() {
        let (ctx, id, depositor) = v1_vault();
        let client = VaultClient::new(&ctx.env, &id);
        let mut runner = MigrationRunner::with_client(&ctx.env, &id, client);
        assert_eq!(runner.current_version(), 1);

        let v2 = runner.execute_migration(2, |client| client.migrate_v1_to_v2());
        assert_eq!((v2.from, v2.to), (1, 2));
        v2.assert_schema_before(&["balance"]);
        v2.assert_schema_after(&["ledger", "version"]);
        v2.assert_renamed("balance", "ledger");
        assert_eq!(runner.current_version(), 2);
        assert_eq!(runner.client().balance_v2(&depositor), 1_000);

        let v3 = runner.execute_migration(3, |client| client.migrate_v2_to_v3());
        v3.assert_added("fee_bps");
        assert_eq!(v3.diff().modified()[0].key, "version");
        assert_eq!(runner.current_version(), 3);
        runner.assert_schema(&["fee_bps", "ledger", "version"]);

        let steps: Vec<(u32, u32)> = runner.history().iter().map(|r| (r.from, r.to)).collect();
        assert_eq!(steps, vec![(1, 2), (2, 3)]);
        assert_eq!(runner.client().balance_v2(&depositor), 1_000);
    }

    #[test]
    fn re_running_an_executed_migration_is_refused_without_calling_the_contract() {
        let (ctx, id, _) = v1_vault();
        let mut runner =
            MigrationRunner::with_client(&ctx.env, &id, VaultClient::new(&ctx.env, &id));
        runner.execute_migration(2, |client| client.migrate_v1_to_v2());

        let mut called = false;
        let error = runner
            .try_execute_migration(2, |_| called = true)
            .unwrap_err();

        assert!(!called);
        assert_eq!(
            error,
            MigrationError::AlreadyApplied {
                to: 2,
                current: 2,
                step: 1
            }
        );
        assert!(error.to_string().contains("already executed"));
    }

    #[test]
    #[should_panic(
        expected = "migration to v2 was already executed (step 1 of this runner) and the contract is at v3; running it again would double-apply it"
    )]
    fn execute_migration_panics_on_a_double_migration() {
        let (ctx, id, _) = v1_vault();
        let mut runner =
            MigrationRunner::with_client(&ctx.env, &id, VaultClient::new(&ctx.env, &id));
        runner.execute_migration(2, |client| client.migrate_v1_to_v2());
        runner.execute_migration(3, |client| client.migrate_v2_to_v3());

        runner.execute_migration(2, |client| client.migrate_v1_to_v2());
    }

    #[test]
    fn skipping_a_version_is_refused() {
        let (ctx, id, _) = v1_vault();
        let mut runner = MigrationRunner::new(&ctx.env, &id);

        let error = runner.try_execute_migration(3, |_| ()).unwrap_err();

        assert_eq!(error, MigrationError::SkipsVersion { from: 1, to: 3 });
        assert!(error.to_string().contains("skips v2"));
    }

    #[test]
    fn migrating_to_the_current_version_is_refused() {
        let (ctx, id, _) = v1_vault();
        let mut runner = MigrationRunner::new(&ctx.env, &id);

        assert_eq!(
            runner.try_execute_migration(1, |_| ()).unwrap_err(),
            MigrationError::NotForward { from: 1, to: 1 }
        );
    }

    #[test]
    fn a_failed_migration_rolls_back_and_is_reported_as_atomic() {
        let (ctx, id, _) = v1_vault();
        let mut runner =
            MigrationRunner::with_client(&ctx.env, &id, VaultClient::new(&ctx.env, &id));
        let before = runner.snapshot();

        let error = runner
            .try_execute_migration(2, |client| client.migrate_v1_to_v2_broken())
            .unwrap_err();

        assert!(
            matches!(error, MigrationError::Failed { from: 1, to: 2, .. }),
            "{error}"
        );
        before.assert_unchanged(&runner.snapshot());
        assert_eq!(runner.current_version(), 1);
        assert!(runner.history().is_empty());

        // The real migration still runs afterwards.
        runner.execute_migration(2, |client| client.migrate_v1_to_v2());
    }

    #[test]
    fn a_step_that_half_writes_storage_is_flagged_as_not_atomic() {
        let (ctx, id, _) = v1_vault();
        let env = ctx.env.clone();
        let mut runner = MigrationRunner::new(&ctx.env, &id);

        // Writing outside a contract call bypasses the host's rollback, which
        // is what a multi-call migration with no transaction around it does.
        let error = runner
            .try_execute_migration(2, |contract| {
                env.as_contract(contract, || {
                    env.storage().persistent().set(&LEDGER, &0u32);
                });
                panic!("second call failed");
            })
            .unwrap_err();

        match &error {
            MigrationError::NotAtomic {
                leftover, message, ..
            } => {
                assert_eq!(leftover, "+ persistent:ledger");
                assert_eq!(message, "second call failed");
            }
            other => panic!("expected NotAtomic, got {other:?}"),
        }
        assert!(error.to_string().contains("partially migrated"));
    }

    #[test]
    fn a_migration_that_forgets_its_version_is_refused() {
        let (ctx, id, _) = v1_vault();
        let mut runner =
            MigrationRunner::with_client(&ctx.env, &id, VaultClient::new(&ctx.env, &id));

        let error = runner
            .try_execute_migration(2, |client| client.migrate_v1_to_v2_unversioned())
            .unwrap_err();

        assert_eq!(
            error,
            MigrationError::VersionNotRecorded {
                expected: 2,
                found: None
            }
        );
    }

    #[contracttype]
    pub enum DataKey {
        Version,
    }

    #[contract]
    pub struct Keyed;

    #[contractimpl]
    impl Keyed {
        pub fn init(env: Env) {
            env.storage().instance().set(&DataKey::Version, &5u32);
        }

        pub fn upgrade(env: Env) {
            env.storage().instance().set(&DataKey::Version, &6u32);
        }
    }

    #[test]
    fn a_custom_version_key_is_read_from_instance_storage() {
        let env = Env::default();
        let id = env.register(Keyed, ());
        let client = KeyedClient::new(&env, &id);
        client.init();

        let mut runner =
            MigrationRunner::with_client(&env, &id, client).with_version_key(DataKey::Version);
        assert_eq!(runner.current_version(), 5);

        runner.execute_migration(6, |client| client.upgrade());
        assert_eq!(runner.stored_version(), Some(6));
    }

    #[test]
    fn the_unversioned_default_can_be_changed() {
        let (ctx, id, _) = v1_vault();
        let runner = MigrationRunner::new(&ctx.env, &id).assuming_unversioned_is(0);

        assert_eq!(runner.current_version(), 0);
    }
}
