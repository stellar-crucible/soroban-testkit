//! Drive in-place contract upgrades from a test and check what survives them.
//!
//! A Soroban contract upgrades itself by calling
//! `env.deployer().update_current_contract_wasm(new_wasm_hash)`. The contract
//! keeps its address and its storage; only the code behind it changes. A
//! production upgrade can still go wrong in four ways, and
//! [`ContractUpgradeHarness`] checks each of them:
//!
//! - **State retention.** Instance and persistent entries written by V1 must
//!   still be there, byte for byte, after V2 takes over
//!   ([`ContractUpgradeHarness::verify_state_preserved`]).
//! - **Schema migration.** A migration entry point must rewrite legacy data
//!   into the new layout, and a migration that fails must leave storage as it
//!   found it ([`ContractUpgradeHarness::try_migrate`]).
//! - **Deprecated functions.** Entry points V2 dropped must stop answering
//!   ([`ContractUpgradeHarness::assert_function_removed`]).
//! - **Authorization.** An upgrade attempted without the required signature
//!   must be refused and leave the old code in place
//!   ([`ContractUpgradeHarness::try_upgrade_to_wasm`]).
//!
//! The harness needs no compiled Wasm. [`ContractUpgradeHarness::upload`]
//! registers a contract type from the current crate under a Wasm hash with
//! `Env::upload`, and [`deploy_uploaded`] deploys V1 the same way, so the
//! upgrade goes through the real `update_current_contract_wasm` host path and
//! calls are dispatched to whichever version the instance now points at.
//!
//! ```ignore
//! let v1 = deploy_uploaded(&ctx.env, TokenV1);
//! let mut harness = ContractUpgradeHarness::with_client(&ctx.env, &v1, TokenV1Client::new(&ctx.env, &v1));
//! harness.populate_v1_state(|client| client.set_config(&initial_config));
//!
//! let v2_wasm_hash = harness.upload(TokenV2);
//! harness.upgrade_to_wasm(&v2_wasm_hash);
//! harness.verify_state_preserved();
//! ```

use std::fmt;
use std::panic::{self, AssertUnwindSafe};

use soroban_sdk::testutils::{Address as _, ContractFunctionSet};
use soroban_sdk::{Address, BytesN, Env, Error, IntoVal, Symbol, Val, Vec as SorobanVec};
use soroban_testkit_core::storage::{StorageDiff, StorageEntry, StorageSnapshot, StorageTier};

/// Deploy a contract type from the current crate as if it were uploaded Wasm.
///
/// The type is uploaded with `Env::upload` and deployed through the deployer,
/// so the instance's executable is a Wasm hash that a later
/// `update_current_contract_wasm` can replace. Deployment needs the deployer's
/// authorization, so `env` must mock authorizations (as
/// [`crate::TestContext::new`] does) when this is called.
pub fn deploy_uploaded<V>(env: &Env, version: V) -> Address
where
    V: ContractFunctionSet + 'static,
{
    let wasm_hash = env.upload(version);
    let deployer = Address::generate(env);
    env.deployer()
        .with_address(deployer, BytesN::from_array(env, &[0; 32]))
        .deploy_v2(wasm_hash, ())
}

/// Why an upgrade or migration step was refused or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpgradeError {
    /// The contract's upgrade entry point refused the call, for example
    /// because the caller lacked authorization. The old code stays in place.
    Rejected { function: String, error: String },
    /// A migration panicked. `atomic` says whether storage was left exactly as
    /// it was; `leftover` describes what changed when it was not.
    MigrationFailed {
        message: String,
        atomic: bool,
        leftover: String,
    },
}

impl fmt::Display for UpgradeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpgradeError::Rejected { function, error } => write!(
                f,
                "the contract refused `{function}`: {error}; the previous code is still in place"
            ),
            UpgradeError::MigrationFailed {
                message, atomic, ..
            } if *atomic => write!(
                f,
                "the migration failed and storage was rolled back: {message}"
            ),
            UpgradeError::MigrationFailed {
                message, leftover, ..
            } => write!(
                f,
                "the migration failed and left storage half-migrated ({leftover}): {message}"
            ),
        }
    }
}

impl std::error::Error for UpgradeError {}

/// One upgrade the harness performed, with storage on either side of it.
#[derive(Clone, Debug)]
pub struct UpgradeRecord {
    pub wasm_hash: BytesN<32>,
    pub before: StorageSnapshot,
    pub after: StorageSnapshot,
}

impl UpgradeRecord {
    /// What the upgrade call changed in storage. Swapping code changes none of
    /// it; a non-empty diff means the upgrade entry point wrote data itself.
    pub fn diff(&self) -> StorageDiff {
        self.before.diff(&self.after)
    }
}

/// A completed migration step and the storage it rewrote.
#[derive(Clone, Debug)]
pub struct MigrationOutcome<R> {
    pub value: R,
    pub before: StorageSnapshot,
    pub after: StorageSnapshot,
}

impl<R> MigrationOutcome<R> {
    pub fn diff(&self) -> StorageDiff {
        self.before.diff(&self.after)
    }

    /// Panic unless `key` existed before the migration and is gone after it.
    pub fn assert_removed(&self, key: &str) {
        self.before.assert_entry_removed(&self.after, key);
    }

    /// Panic unless `key` is new after the migration.
    pub fn assert_added(&self, key: &str) {
        self.before.assert_entry_added(&self.after, key);
    }
}

/// Orchestrates V1 state, an in-place upgrade, migrations, and the checks
/// that state survived them. See the [module docs](self).
///
/// The harness keeps its own handle to the environment (`Env` is a cheap,
/// shared handle), so it does not borrow the test context.
pub struct ContractUpgradeHarness<C = Address> {
    env: Env,
    contract: Address,
    client: C,
    upgrade_fn: String,
    baseline: Option<StorageSnapshot>,
    history: Vec<UpgradeRecord>,
}

impl ContractUpgradeHarness<Address> {
    /// A harness for `contract`, without a typed client.
    pub fn new(env: &Env, contract: &Address) -> Self {
        Self::with_client(env, contract, contract.clone())
    }
}

impl<C> ContractUpgradeHarness<C> {
    /// A harness that hands `client` to the closures it runs.
    pub fn with_client(env: &Env, contract: &Address, client: C) -> Self {
        Self {
            env: env.clone(),
            contract: contract.clone(),
            client,
            upgrade_fn: "upgrade".to_string(),
            baseline: None,
            history: Vec::new(),
        }
    }

    /// Use `name` as the upgrade entry point instead of `upgrade`. It must take
    /// the new Wasm hash as its only argument.
    pub fn with_upgrade_function(mut self, name: &str) -> Self {
        self.upgrade_fn = name.to_string();
        self
    }

    pub fn contract(&self) -> &Address {
        &self.contract
    }

    pub fn client(&self) -> &C {
        &self.client
    }

    /// Register `version` under a fresh Wasm hash and return the hash, ready
    /// for [`Self::upgrade_to_wasm`].
    pub fn upload<V>(&self, version: V) -> BytesN<32>
    where
        V: ContractFunctionSet + 'static,
    {
        self.env.upload(version)
    }

    /// Capture the contract's live storage.
    pub fn snapshot(&self) -> StorageSnapshot {
        StorageSnapshot::capture(&self.env, &self.contract)
    }

    /// Run `populate` against V1 and record the resulting storage as the
    /// baseline that [`Self::verify_state_preserved`] compares against.
    pub fn populate_v1_state<R>(&mut self, populate: impl FnOnce(&C) -> R) -> R {
        let value = populate(&self.client);
        self.baseline = Some(self.snapshot());
        value
    }

    /// Record the current storage as the baseline.
    pub fn mark_baseline(&mut self) -> &StorageSnapshot {
        let snapshot = self.snapshot();
        self.baseline.insert(snapshot)
    }

    pub fn baseline(&self) -> Option<&StorageSnapshot> {
        self.baseline.as_ref()
    }

    /// Every upgrade performed so far, oldest first.
    pub fn history(&self) -> &[UpgradeRecord] {
        &self.history
    }

    /// Call the contract's upgrade entry point with `wasm_hash`.
    ///
    /// The call runs with whatever authorization the environment provides, so
    /// a test can withhold it (for example with `env.set_auths(&[])`) to check
    /// that an unauthorized upgrade is refused.
    pub fn try_upgrade_to_wasm(
        &mut self,
        wasm_hash: &BytesN<32>,
    ) -> Result<&UpgradeRecord, UpgradeError> {
        let before = self.snapshot();
        let args: SorobanVec<Val> = (wasm_hash.clone(),).into_val(&self.env);
        let function = Symbol::new(&self.env, &self.upgrade_fn);
        let outcome = self
            .env
            .try_invoke_contract::<Val, Error>(&self.contract, &function, args);
        let failure = match outcome {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(format!("{error:?}")),
            Err(Ok(error)) => Some(format!("{error:?}")),
            Err(Err(error)) => Some(format!("{error:?}")),
        };
        if let Some(error) = failure {
            return Err(UpgradeError::Rejected {
                function: self.upgrade_fn.clone(),
                error,
            });
        }
        let after = self.snapshot();
        self.history.push(UpgradeRecord {
            wasm_hash: wasm_hash.clone(),
            before,
            after,
        });
        Ok(self.history.last().expect("an upgrade was just recorded"))
    }

    /// [`Self::try_upgrade_to_wasm`], panicking with a diagnostic on failure.
    pub fn upgrade_to_wasm(&mut self, wasm_hash: &BytesN<32>) -> &UpgradeRecord {
        if let Err(error) = self.try_upgrade_to_wasm(wasm_hash) {
            panic!("{error}");
        }
        self.history.last().expect("an upgrade was just recorded")
    }

    /// Panic unless every instance and persistent entry of the baseline is
    /// still present with the same value.
    pub fn verify_state_preserved(&self) {
        self.verify_state_preserved_except(&[]);
    }

    /// [`Self::verify_state_preserved`], skipping keys a migration is expected
    /// to rewrite or remove. Temporary entries are not checked: they may
    /// expire independently of any upgrade.
    pub fn verify_state_preserved_except(&self, migrated: &[&str]) {
        let baseline = self.baseline.as_ref().unwrap_or_else(|| {
            panic!(
                "no baseline to compare against; call populate_v1_state or mark_baseline before upgrading"
            )
        });
        let now = self.snapshot();
        let problems = preservation_problems(baseline, &now, migrated);
        assert!(
            problems.is_empty(),
            "state was not preserved across the upgrade:\n  {}",
            problems.join("\n  ")
        );
    }

    /// Run a migration step and report whether a failure rolled back cleanly.
    pub fn try_migrate<R>(
        &mut self,
        migrate: impl FnOnce(&C) -> R,
    ) -> Result<MigrationOutcome<R>, UpgradeError> {
        let before = self.snapshot();
        let client = &self.client;
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| migrate(client)));
        let after = self.snapshot();
        match outcome {
            Ok(value) => Ok(MigrationOutcome {
                value,
                before,
                after,
            }),
            Err(payload) => {
                let diff = before.diff(&after);
                Err(UpgradeError::MigrationFailed {
                    message: panic_message(payload.as_ref()),
                    atomic: diff.is_empty(),
                    leftover: describe(&diff),
                })
            }
        }
    }

    /// [`Self::try_migrate`], panicking with a diagnostic on failure.
    pub fn migrate<R>(&mut self, migrate: impl FnOnce(&C) -> R) -> MigrationOutcome<R> {
        match self.try_migrate(migrate) {
            Ok(outcome) => outcome,
            Err(error) => panic!("{error}"),
        }
    }

    /// Panic unless calling `function` with `args` now fails: an entry point
    /// the new version dropped must not keep answering.
    pub fn assert_function_removed(&self, function: &str, args: SorobanVec<Val>) {
        let outcome = self.env.try_invoke_contract::<Val, Error>(
            &self.contract,
            &Symbol::new(&self.env, function),
            args,
        );
        assert!(
            outcome.is_err(),
            "deprecated function `{function}` still answers after the upgrade"
        );
    }
}

fn preservation_problems(
    baseline: &StorageSnapshot,
    now: &StorageSnapshot,
    migrated: &[&str],
) -> Vec<String> {
    let find = |entry: &StorageEntry| {
        now.entries()
            .iter()
            .find(|candidate| candidate.tier == entry.tier && candidate.key == entry.key)
    };
    baseline
        .entries()
        .iter()
        .filter(|entry| entry.tier != StorageTier::Temporary)
        .filter(|entry| !migrated.contains(&entry.key.as_str()))
        .filter_map(|entry| match find(entry) {
            None => Some(format!("{} was lost", entry.qualified())),
            Some(current) if current.bytes != entry.bytes => Some(format!(
                "{} changed from {} to {}",
                entry.qualified(),
                entry.value,
                current.value
            )),
            Some(_) => None,
        })
        .collect()
}

fn describe(diff: &StorageDiff) -> String {
    let mut parts = Vec::new();
    parts.extend(
        diff.added()
            .iter()
            .map(|e| format!("added {}", e.qualified())),
    );
    parts.extend(
        diff.removed()
            .iter()
            .map(|e| format!("removed {}", e.qualified())),
    );
    parts.extend(
        diff.modified()
            .iter()
            .map(|c| format!("modified {}:{}", c.tier.as_str(), c.key)),
    );
    parts.join(", ")
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestContext;
    use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, vec};

    const ADMIN: Symbol = symbol_short!("admin");
    const CONFIG: Symbol = symbol_short!("config");
    const CONFIG2: Symbol = symbol_short!("config2");
    const NOTE: Symbol = symbol_short!("note");
    const VERSION: Symbol = symbol_short!("version");

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum Key {
        Balance(Address),
    }

    /// The V2 configuration layout; V1 stored a bare fee as `u32`.
    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct ConfigV2 {
        pub fee_bps: u32,
        pub max_deposit: i128,
    }

    fn upgrade_code(env: &Env, new_wasm_hash: BytesN<32>) {
        let admin: Address = env.storage().instance().get(&ADMIN).unwrap();
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    #[contract]
    pub struct VaultV1;

    #[contractimpl]
    impl VaultV1 {
        pub fn init(env: Env, admin: Address) {
            env.storage().instance().set(&ADMIN, &admin);
            env.storage().instance().set(&VERSION, &1u32);
        }

        pub fn set_config(env: Env, fee_bps: u32) {
            env.storage().instance().set(&CONFIG, &fee_bps);
        }

        pub fn deposit(env: Env, who: Address, amount: i128) {
            env.storage().persistent().set(&Key::Balance(who), &amount);
        }

        pub fn note(env: Env, value: u32) {
            env.storage().temporary().set(&NOTE, &value);
        }

        pub fn balance(env: Env, who: Address) -> i128 {
            env.storage()
                .persistent()
                .get(&Key::Balance(who))
                .unwrap_or(0)
        }

        /// Dropped in V2.
        pub fn legacy_ping(_env: Env) -> u32 {
            1
        }

        pub fn code_version(_env: Env) -> u32 {
            1
        }

        pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
            upgrade_code(&env, new_wasm_hash);
        }
    }

    #[contract]
    pub struct VaultV2;

    #[contractimpl]
    impl VaultV2 {
        pub fn balance(env: Env, who: Address) -> i128 {
            env.storage()
                .persistent()
                .get(&Key::Balance(who))
                .unwrap_or(0)
        }

        pub fn code_version(_env: Env) -> u32 {
            2
        }

        pub fn config(env: Env) -> ConfigV2 {
            env.storage().instance().get(&CONFIG2).unwrap()
        }

        /// Converts the legacy `u32` fee into the `ConfigV2` layout.
        pub fn migrate(env: Env) {
            let fee_bps: u32 = env.storage().instance().get(&CONFIG).unwrap();
            env.storage().instance().set(
                &CONFIG2,
                &ConfigV2 {
                    fee_bps,
                    max_deposit: 1_000_000,
                },
            );
            env.storage().instance().remove(&CONFIG);
            env.storage().instance().set(&VERSION, &2u32);
        }

        /// Writes the new layout, then fails: the host rolls the call back.
        pub fn migrate_broken(env: Env) {
            env.storage().instance().set(
                &CONFIG2,
                &ConfigV2 {
                    fee_bps: 0,
                    max_deposit: 0,
                },
            );
            panic!("legacy config is corrupt");
        }

        /// Stage one of a two-call migration; completes on its own.
        pub fn migrate_stage_one(env: Env) {
            env.storage().instance().set(&VERSION, &2u32);
        }

        /// Loses a depositor's balance, which the preservation check catches.
        pub fn forget(env: Env, who: Address) {
            env.storage().persistent().remove(&Key::Balance(who));
        }

        pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
            upgrade_code(&env, new_wasm_hash);
        }
    }

    struct Setup {
        ctx: TestContext,
        id: Address,
        depositor: Address,
    }

    impl Setup {
        fn v1(&self) -> VaultV1Client<'_> {
            VaultV1Client::new(&self.ctx.env, &self.id)
        }

        fn v2(&self) -> VaultV2Client<'_> {
            VaultV2Client::new(&self.ctx.env, &self.id)
        }
    }

    /// V1 deployed as uploaded code, with one entry in every storage tier.
    fn v1_with_state() -> (Setup, ContractUpgradeHarness) {
        let mut ctx = TestContext::new();
        let depositor = ctx.add_user().clone();
        let id = deploy_uploaded(&ctx.env, VaultV1);
        let mut harness = ContractUpgradeHarness::new(&ctx.env, &id);
        let client = VaultV1Client::new(&ctx.env, &id);
        harness.populate_v1_state(|_| {
            client.init(&ctx.admin);
            client.set_config(&30);
            client.deposit(&depositor, &500);
            client.note(&7);
        });
        (Setup { ctx, id, depositor }, harness)
    }

    #[test]
    fn a_valid_upgrade_switches_code_and_preserves_every_tier() {
        let (setup, mut harness) = v1_with_state();
        assert_eq!(setup.v1().code_version(), 1);
        let baseline = harness.baseline().unwrap().clone();
        assert!(baseline.get("config").is_some());
        assert_eq!(baseline.in_tier(StorageTier::Persistent).len(), 1);
        assert_eq!(baseline.in_tier(StorageTier::Temporary).len(), 1);

        let v2 = harness.upload(VaultV2);
        let record = harness.upgrade_to_wasm(&v2);
        assert!(
            record.diff().is_empty(),
            "swapping code must not touch storage"
        );
        assert_eq!(record.wasm_hash, v2);

        harness.verify_state_preserved();
        assert_eq!(setup.v2().code_version(), 2);
        assert_eq!(setup.v2().balance(&setup.depositor), 500);
        assert_eq!(harness.snapshot().in_tier(StorageTier::Temporary).len(), 1);
        assert_eq!(harness.history().len(), 1);
    }

    #[test]
    fn a_migration_converts_the_legacy_schema() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        harness.upgrade_to_wasm(&v2);

        let outcome = harness.migrate(|_| setup.v2().migrate());
        outcome.assert_removed("config");
        outcome.assert_added("config2");
        assert_eq!(
            setup.v2().config(),
            ConfigV2 {
                fee_bps: 30,
                max_deposit: 1_000_000
            }
        );
        // Everything except the keys the migration owns is untouched.
        harness.verify_state_preserved_except(&["config", "version"]);
    }

    #[test]
    fn a_failed_migration_rolls_back_atomically() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        harness.upgrade_to_wasm(&v2);

        let error = harness
            .try_migrate(|_| setup.v2().migrate_broken())
            .unwrap_err();
        assert!(matches!(
            error,
            UpgradeError::MigrationFailed { atomic: true, .. }
        ));
        assert!(error.to_string().contains("rolled back"));
        harness.verify_state_preserved();
    }

    #[test]
    fn a_migration_split_across_calls_reports_half_migrated_state() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        harness.upgrade_to_wasm(&v2);

        let error = harness
            .try_migrate(|_| {
                setup.v2().migrate_stage_one();
                setup.v2().migrate_broken();
            })
            .unwrap_err();
        match &error {
            UpgradeError::MigrationFailed {
                atomic, leftover, ..
            } => {
                assert!(!atomic);
                assert!(leftover.contains("modified instance:version"), "{leftover}");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(error.to_string().contains("half-migrated"));
    }

    #[test]
    fn an_unauthorized_upgrade_is_refused_and_keeps_v1() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        setup.ctx.env.set_auths(&[]);

        let error = harness.try_upgrade_to_wasm(&v2).unwrap_err();
        assert!(matches!(error, UpgradeError::Rejected { .. }));
        assert!(error.to_string().contains("still in place"));
        assert!(harness.history().is_empty());

        setup.ctx.env.mock_all_auths();
        assert_eq!(setup.v1().code_version(), 1);
        harness.verify_state_preserved();
    }

    #[test]
    fn deprecated_functions_stop_answering_after_the_upgrade() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        harness.upgrade_to_wasm(&v2);
        harness.assert_function_removed("legacy_ping", vec![&setup.ctx.env]);
    }

    #[test]
    #[should_panic(expected = "deprecated function `legacy_ping` still answers")]
    fn a_function_that_still_exists_fails_the_removal_check() {
        let (setup, harness) = v1_with_state();
        assert_eq!(setup.v1().legacy_ping(), 1);
        harness.assert_function_removed("legacy_ping", vec![&setup.ctx.env]);
    }

    #[test]
    #[should_panic(expected = "was lost")]
    fn losing_persistent_state_fails_the_preservation_check() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        harness.upgrade_to_wasm(&v2);
        setup.v2().forget(&setup.depositor);
        harness.verify_state_preserved();
    }

    #[test]
    #[should_panic(expected = "no baseline to compare against")]
    fn verifying_without_a_baseline_is_a_usage_error() {
        let ctx = TestContext::new();
        let id = deploy_uploaded(&ctx.env, VaultV1);
        ContractUpgradeHarness::new(&ctx.env, &id).verify_state_preserved();
    }

    #[test]
    fn a_missing_upgrade_entry_point_is_reported() {
        let ctx = TestContext::new();
        let id = deploy_uploaded(&ctx.env, VaultV1);
        VaultV1Client::new(&ctx.env, &id).init(&ctx.admin);
        let mut harness =
            ContractUpgradeHarness::new(&ctx.env, &id).with_upgrade_function("upgrade_code");
        assert_eq!(harness.contract(), &id);
        harness.mark_baseline();
        let v2 = harness.upload(VaultV2);
        let error = harness.try_upgrade_to_wasm(&v2).unwrap_err();
        assert!(error.to_string().contains("`upgrade_code`"));
        harness.verify_state_preserved();
    }

    #[test]
    #[should_panic(expected = "the contract refused `upgrade`")]
    fn upgrade_to_wasm_panics_with_the_reason() {
        let (setup, mut harness) = v1_with_state();
        let v2 = harness.upload(VaultV2);
        setup.ctx.env.set_auths(&[]);
        harness.upgrade_to_wasm(&v2);
    }
}
