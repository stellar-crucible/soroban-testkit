extern crate self as soroban_testkit_fixtures;

pub mod archival;
pub mod branching;
pub mod builder;
pub mod checkpoint;
pub mod congestion;
pub mod consensus;
pub mod crypto;
pub mod deterministic_deployer;
pub mod event_indexer;
pub mod eviction;
pub mod failure_injector;
pub mod gas_exhaustion;
pub mod migration;
pub mod mock;
pub mod oracle;
pub mod profiles;
pub mod protocol_registry;
pub mod reentrancy;
pub mod storage_compaction;
pub mod topology;
#[cfg(test)]
mod ttl_tests;
pub mod upgrade;

pub use branching::BranchingContext;
pub use builder::TestContextBuilder;
pub use checkpoint::ContextCheckpoint;
pub use congestion::{
    CongestionSimulator, DelayReport, DEFAULT_BASE_FEE_STROOPS, DEFAULT_SECONDS_PER_LEDGER,
};
pub use consensus::{
    ConsensusRound, ConsensusSimulator, LedgerClose, DEFAULT_JITTER_STD_MS,
    DEFAULT_MAX_CLOSE_INTERVAL_MS, DEFAULT_MIN_CLOSE_INTERVAL_MS, DEFAULT_QUORUM, DEFAULT_SEED,
    DEFAULT_VALIDATORS, QUORUM_ACCEPTANCE_SIGMA,
};
pub use deterministic_deployer::{DeterministicDeployer, SaltHierarchy};

pub use migration::{MigrationError, MigrationRecord, MigrationRunner};
pub use mock::{MockContract, MockContractBuilder, MockContractType};
pub use oracle::{
    AssetPrice, MockPriceOracle, MockPriceOracleClient, OracleError, OracleHarness, Sep40Asset,
    Sep40PriceData,
};
pub use profiles::NetworkProfile;
pub use storage_compaction::{StorageCompactionReport, StorageCompactor};
pub use protocol_registry::{
    ConsumerContract, ProtocolRegistryContract, ProtocolRegistryContractClient,
    ProtocolRegistryError, ProtocolRegistryHarness, ProtocolUpgradeLogEntry, ServiceRecord,
    ServiceRole, ServiceUpgradeRecord, UpgradeProxyContract, UpgradeProxyContractClient,
};
pub use soroban_testkit_fixtures_macros::mock_contract;
pub use topology::{
    AddressReference, ContractNode, DanglingReference, DependencyLink, InactiveContract,
    TopologyReport, TopologyValidator, UninitializedLink,
};
pub use upgrade::{
    deploy_uploaded, ContractUpgradeHarness, MigrationOutcome, UpgradeError, UpgradeRecord,
};

pub use failure_injector::{failure_point, FailureInjector, InjectedFailure};

use soroban_env_host::HostError;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::xdr::ScAddress;
use soroban_sdk::{Address, Env, IntoVal, TryFromVal as _, Val};
pub use soroban_testkit_core::ttl::{RentThresholds, TtlExtension, TtlInspector, TtlStatus};
use soroban_testkit_core::StorageTier;

pub use event_indexer::{
    EventIdentity, EventIndexerEmulator, EventIndexerError, EventPage, EventQuery, IndexedEvent,
};
pub use eviction::StateEvictionSimulator;
pub use gas_exhaustion::{
    ExecutionBudget, ExecutionReport, ExecutionStatus, ExhaustedResource, GasExhaustionHarness,
};

pub struct TestContext {
    pub env: Env,
    pub admin: Address,
    pub users: Vec<Address>,
    /// The auth policy [`TestContext::reset`] re-applies to the fresh env.
    ///
    /// The SDK cannot read an env's auth mode back, so a context wraps an
    /// assumed policy: `new` and the builder record what they actually did,
    /// and `with_env` defaults to mocked.
    pub mock_auths: bool,
    /// The network conditions the builder installed and [`TestContext::reset`]
    /// re-applies to the fresh env.
    pub network_profile: NetworkProfile,
    /// Stack of checkpoints for nested rollback support.
    checkpoint_stack: Vec<ContextCheckpoint>,
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

impl TestContext {
    pub fn new() -> Self {
        let env = Env::default();
        env.mock_all_auths();
        Self::with_env(env)
    }

    /// Builds a context around an environment the caller already configured,
    /// so every address in the context belongs to that environment.
    ///
    /// An `Env` does not report whether it mocks authorizations, so this
    /// assumes it does — the same assumption [`TestContext::new`] makes. Set
    /// [`TestContext::mock_auths`] to `false` afterwards if it does not, so
    /// [`TestContext::reset`] reproduces your policy rather than adding one.
    pub fn with_env(env: Env) -> Self {
        let admin = Address::generate(&env);
        Self {
            env,
            admin,
            users: Vec::new(),
            mock_auths: true,
            network_profile: NetworkProfile::Local,
            checkpoint_stack: Vec::new(),
        }
    }

    pub fn add_user(&mut self) -> &Address {
        let user = Address::generate(&self.env);
        self.users.push(user);
        self.users.last().unwrap()
    }

    /// Returns a reference to the user address at `index`.
    ///
    /// # Panics
    ///
    /// Panics with a diagnostic message if `index` is out of bounds, naming
    /// the requested index, the actual user count, and pointing at
    /// [`crate::builder::TestContextBuilder::with_users`].
    pub fn user(&self, index: usize) -> &Address {
        self.users.get(index).unwrap_or_else(|| {
            panic!(
                "user index out of bounds: index is {} but context holds {} user{} (did you call TestContextBuilder::with_users({})?)",
                index,
                self.users.len(),
                if self.users.len() == 1 { "" } else { "s" },
                index + 1
            );
        })
    }

    /// The timestamp the ledger currently reports.
    pub fn timestamp(&self) -> u64 {
        self.env.ledger().timestamp()
    }

    /// The sequence number the ledger currently reports.
    pub fn sequence(&self) -> u32 {
        self.env.ledger().sequence()
    }

    /// Move the ledger clock to an absolute unix timestamp.
    ///
    /// Prefer [`TestContext::advance_time`] when the interesting property is
    /// the elapsed span; an absolute timestamp pins the test to a moment in
    /// history and breaks the day the fixture is read next year.
    pub fn set_timestamp(&mut self, timestamp: u64) {
        self.env.ledger().set_timestamp(timestamp);
    }

    /// Move the ledger clock forward by `seconds`, returning the new timestamp.
    ///
    /// The sequence number does not move with it: a contract that reads
    /// `env.ledger().sequence()` sees no ledger close at all. Call
    /// [`TestContext::advance_ledger`] as well when the test depends on both,
    /// which is what a real chain does — every close advances both.
    pub fn advance_time(&mut self, seconds: u64) -> u64 {
        let next = self.timestamp().saturating_add(seconds);
        self.env.ledger().set_timestamp(next);
        next
    }

    /// Jump `count` ledgers forward, returning the new sequence number.
    ///
    /// The timestamp stays where it was, so the clock does not silently advance
    /// by the network's five-second cadence when a test only wanted entries to
    /// expire.
    pub fn advance_ledger(&mut self, count: u32) -> u32 {
        let next = self.sequence().saturating_add(count);
        self.env.ledger().set_sequence_number(next);
        next
    }

    /// Removes expired contract storage entries from the mock host.
    ///
    /// Advance the ledger first with [`TestContext::advance_ledger`]. Expired
    /// temporary entries become absent, while expired persistent entries are
    /// removed from the mock host to simulate archival.
    ///
    /// # Errors
    ///
    /// Returns a host error if storage compaction fails.
    pub fn prune_expired_storage(&self) -> Result<StorageCompactionReport, HostError> {
        StorageCompactor::new(&self.env).prune_expired()
    }
    /// Archive every entry this context's ledger has let expire, returning the
    /// simulator that holds them.
    ///
    /// Moves the ledger `count` sequences forward first, then sweeps the whole
    /// environment the way the network does between ledgers: persistent
    /// entries past their TTL are archived (and can be restored), temporary
    /// ones are evicted for good. Contract instance and code entries stay live,
    /// so the contracts registered here remain callable while the test reads
    /// what the sweep did to their data.
    ///
    /// ```rust
    /// use soroban_testkit_fixtures::builder::TestContextBuilder;
    ///
    /// let mut ctx = TestContextBuilder::new().build();
    /// // With nothing stored, the sweep has nothing to archive.
    /// let simulator = ctx.archive_expired(10_000);
    /// assert!(simulator.is_empty());
    /// ```
    pub fn archive_expired(&mut self, count: u32) -> crate::archival::ArchivalSweepSimulator {
        let mut simulator = crate::archival::ArchivalSweepSimulator::new();
        simulator.sweep(&self.env, count);
        simulator
    }

    /// The sequence number the ledger currently reports; the same reading as
    /// [`TestContext::sequence`], named for TTL arithmetic.
    pub fn ledger_sequence(&self) -> u32 {
        self.sequence()
    }

    /// Inspect the live TTL and rent thresholds of the entry `contract` stores
    /// under `key`, in whichever tier holds it.
    ///
    /// See [`TtlInspector::for_key`] for how the tier is chosen.
    pub fn ttl<K: IntoVal<Env, Val>>(&self, contract: &Address, key: &K) -> TtlInspector {
        TtlInspector::for_key(&self.env, contract, key)
    }

    /// Move the ledger to the first sequence at which `key`'s entry is no
    /// longer live, returning that sequence.
    ///
    /// The target is read from the entry's own `live_until_ledger`, so the
    /// test never hard-codes the network's minimum rent. Afterwards a
    /// temporary entry reads as empty and a persistent entry (or an instance
    /// key) is archived.
    ///
    /// ```rust
    /// use soroban_sdk::{contract, contractimpl, symbol_short, Env, Val};
    /// use soroban_testkit_fixtures::TestContext;
    /// use soroban_testkit_fixtures::TtlInspector;
    ///
    /// #[contract]
    /// pub struct Nonces;
    ///
    /// #[contractimpl]
    /// impl Nonces {
    ///     pub fn put(env: Env) {
    ///         env.storage().temporary().set(&symbol_short!("nonce"), &1u32);
    ///     }
    /// }
    ///
    /// # fn main() {
    /// let ctx = TestContext::new();
    /// let contract_id = ctx.env.register(Nonces, ());
    /// NoncesClient::new(&ctx.env, &contract_id).put();
    /// let key = symbol_short!("nonce");
    ///
    /// let ttl = TtlInspector::for_key(&ctx.env, &contract_id, &key);
    /// assert!(ttl.live_until_ledger > ctx.ledger_sequence());
    ///
    /// ctx.fast_forward_to_expiration(&contract_id, &key);
    /// let value = ctx.env.as_contract(&contract_id, || {
    ///     ctx.env.storage().temporary().get::<_, Val>(&key)
    /// });
    /// assert!(value.is_none());
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics when no tier of `contract` holds `key`.
    pub fn fast_forward_to_expiration<K: IntoVal<Env, Val>>(
        &self,
        contract: &Address,
        key: &K,
    ) -> u32 {
        let target = self.ttl(contract, key).expiration_ledger();
        self.env.ledger().set_sequence_number(target);
        target
    }

    /// Move the ledger to the last sequence at which `key`'s entry is still
    /// live — the boundary one ledger before
    /// [`TestContext::fast_forward_to_expiration`] — returning that sequence.
    ///
    /// # Panics
    ///
    /// Panics when no tier of `contract` holds `key`.
    pub fn fast_forward_to_last_live_ledger<K: IntoVal<Env, Val>>(
        &self,
        contract: &Address,
        key: &K,
    ) -> u32 {
        let target = self.ttl(contract, key).live_until_ledger;
        self.env.ledger().set_sequence_number(target);
        target
    }

    /// Run `call` and report what it did to `key`'s TTL, for checking an
    /// `extend_ttl` inside a contract against the host's rules.
    ///
    /// # Panics
    ///
    /// Panics when no tier of `contract` holds `key` before the call.
    pub fn observe_ttl_extension<K, F, T>(
        &self,
        contract: &Address,
        key: &K,
        call: F,
    ) -> TtlExtension
    where
        K: IntoVal<Env, Val>,
        F: FnOnce() -> T,
    {
        TtlExtension::observe(&self.env, contract, key, call)
    }

    /// Fail unless `contract`'s temporary entry under `key` is gone: either
    /// past its TTL, or already dropped by the host.
    ///
    /// Proves it the way a contract would see it — by reading the key from
    /// inside `contract` and finding nothing.
    ///
    /// # Panics
    ///
    /// Panics when the entry is still live, or when a read returns a value.
    #[track_caller]
    pub fn assert_temporary_expired<K: IntoVal<Env, Val>>(&self, contract: &Address, key: &K) {
        if let Some(ttl) =
            TtlInspector::try_in_tier(&self.env, contract, key, StorageTier::Temporary)
        {
            ttl.assert_expired();
        }
        let key: Val = key.into_val(&self.env);
        let (has, value) = self.env.as_contract(contract, || {
            let temporary = self.env.storage().temporary();
            (temporary.has(&key), temporary.get::<_, Val>(&key))
        });
        assert!(
            !has && value.is_none(),
            "Expected the temporary entry {key:?} of {contract:?} to read as empty at ledger {}, but the contract can still read it",
            self.ledger_sequence()
        );
    }

    /// Fail unless `contract`'s persistent entry under `key` has passed its
    /// TTL and is therefore archived.
    ///
    /// The check reads the TTL without accessing the entry, because a normal
    /// read in the SDK test host restores an archived entry on the spot. Use
    /// [`TestContext::archive_expired`] to take archived entries off the ledger
    /// entirely and test the restore path.
    ///
    /// # Panics
    ///
    /// Panics when persistent storage does not hold `key`, or when the entry
    /// is still live.
    #[track_caller]
    pub fn assert_persistent_archived<K: IntoVal<Env, Val>>(&self, contract: &Address, key: &K) {
        let ttl = TtlInspector::persistent(&self.env, contract, key);
        assert_eq!(
            ttl.status(),
            TtlStatus::Archived,
            "Expected {} to be archived at ledger {}, but it is live until ledger {}",
            ttl.describe(),
            ttl.current_ledger,
            ttl.live_until_ledger
        );
    }

    /// Swap in a fresh environment and keep every address this context holds.
    ///
    /// The new env starts at ledger 0 with no events, no recorded
    /// authorizations and no contract storage, so a multi-phase test can run
    /// phase two with the same actors on a clean chain instead of tearing the
    /// whole fixture down. Contracts registered on the previous env do *not*
    /// come along — registration belongs to the env that created it, so
    /// re-register after resetting.
    ///
    /// Reach for a second `TestContext` rather than `reset` when the two
    /// phases want different fixtures, different user counts or a different
    /// auth policy; `reset` is for the case where only the chain is supposed
    /// to change.
    pub fn reset(&mut self) {
        let carried: Vec<ScAddress> = std::iter::once(&self.admin)
            .chain(self.users.iter())
            .map(ScAddress::from)
            .collect();

        let env = Env::default();
        if self.mock_auths {
            env.mock_all_auths();
        }
        self.network_profile.apply(&env);
        self.env = env;

        self.admin = migrated_to(&self.env, &carried[0]);
        self.users = carried[1..]
            .iter()
            .map(|carried| migrated_to(&self.env, carried))
            .collect();
    }

    /// Swap in a fresh environment *and* fresh identities: a new admin and a
    /// new `users` list of the same length, none of them an address this
    /// context held before.
    ///
    /// SDK v28 mints test addresses from a counter that restarts at 1 in every
    /// `Env`, so regenerating straight away hands back the addresses you just
    /// replaced. This walks the new env's counter past the range the old
    /// identities occupied, which is the only thing that makes the new ones
    /// new.
    pub fn reset_full(&mut self) {
        let user_count = self.users.len();
        let env = Env::default();
        if self.mock_auths {
            env.mock_all_auths();
        }

        for _ in 0..=user_count {
            let _ = Address::generate(&env);
        }
        self.env = env;

        self.admin = Address::generate(&self.env);
        self.users = (0..user_count)
            .map(|_| Address::generate(&self.env))
            .collect();
    }

    /// Create a checkpoint of the current environment state.
    ///
    /// This captures:
    /// - Storage for all contracts (instance, persistent, temporary)
    /// - Ledger sequence and timestamp
    /// - Mock auth configuration
    /// - Network profile
    /// - Events
    ///
    /// Returns an opaque [`ContextCheckpoint`] that can be used with
    /// [`TestContext::rollback_to`] to restore this exact state.
    ///
    /// Multiple checkpoints can be created and they form a stack.
    /// Use [`TestContext::rollback_to`] to restore to a specific checkpoint.
    pub fn checkpoint(&mut self) -> ContextCheckpoint {
        let checkpoint = crate::checkpoint::ContextCheckpoint::capture(
            &self.env,
            self.mock_auths,
            self.network_profile,
        );
        self.checkpoint_stack.push(checkpoint.clone());
        checkpoint
    }

    /// Roll back the environment to a previously created checkpoint.
    ///
    /// This restores:
    /// - All contract storage (instance, persistent, temporary)
    /// - Ledger sequence and timestamp
    /// - Mock auth configuration
    /// - Network profile
    /// - Events (discards events generated after the checkpoint)
    ///
    /// The checkpoint must have been created by this context's [`checkpoint`] method.
    /// After rollback, all checkpoints created after the target are discarded.
    ///
    /// # Panics
    ///
    /// Panics if the checkpoint was not created by this context.
    pub fn rollback_to(&mut self, checkpoint: &ContextCheckpoint) {
        // Find the checkpoint in our stack
        let idx = self.checkpoint_stack.iter().position(|c| {
            c.ledger_sequence == checkpoint.ledger_sequence
                && c.ledger_timestamp == checkpoint.ledger_timestamp
                && c.mock_auths == checkpoint.mock_auths
        });

        let Some(idx) = idx else {
            panic!("Attempted to rollback to a checkpoint not created by this context");
        };

        // Restore the checkpoint
        checkpoint.restore(&self.env);

        // Restore auth mocking
        if checkpoint.mock_auths {
            self.env.mock_all_auths();
        }

        // Restore network profile
        checkpoint.network_profile.apply(&self.env);

        // Truncate the checkpoint stack to remove checkpoints after the target
        self.checkpoint_stack.truncate(idx + 1);
    }

    /// Get the number of checkpoints in the stack.
    pub fn checkpoint_count(&self) -> usize {
        self.checkpoint_stack.len()
    }

    /// Clear all checkpoints.
    pub fn clear_checkpoints(&mut self) {
        self.checkpoint_stack.clear();
    }

    /// Generates a cryptographically signed authorization entry for testing invocation verification.
    ///
    /// Constructs a valid [`soroban_sdk::testutils::AuthorizedInvocation`] for `contract::function(args)`
    /// and signs the authorization digest using `signer`.
    pub fn generate_signed_auth(
        &self,
        signer: &crypto::SignerKeypair,
        contract: &Address,
        function: soroban_sdk::Symbol,
        args: soroban_sdk::Vec<soroban_sdk::Val>,
    ) -> crypto::SignedAuth {
        self.generate_signed_auth_with_nonce(signer, contract, function, args, 0)
    }

    /// Generates a signed authorization entry with an explicit nonce for replay testing.
    pub fn generate_signed_auth_with_nonce(
        &self,
        signer: &crypto::SignerKeypair,
        contract: &Address,
        function: soroban_sdk::Symbol,
        args: soroban_sdk::Vec<soroban_sdk::Val>,
        nonce: u64,
    ) -> crypto::SignedAuth {
        use soroban_sdk::testutils::{AuthorizedFunction, AuthorizedInvocation};

        let message = crypto::construct_auth_message(&self.env, contract, &function, &args, nonce);
        let signature = signer.sign(&message);
        let invocation = AuthorizedInvocation {
            function: AuthorizedFunction::Contract((contract.clone(), function, args)),
            sub_invocations: Vec::new(),
        };

        crypto::SignedAuth {
            invocation,
            signer: signer.address().clone(),
            signature,
            message,
            nonce,
        }
    }
}

/// Rebuild an address as an object of `env`.
///
/// An `Address` is a handle into the env that created it, so carrying an
/// identity across an env boundary goes through its `ScAddress` form and back.
fn migrated_to(env: &Env, address: &ScAddress) -> Address {
    Address::try_from_val(env, address).unwrap()
}

#[cfg(test)]
mod tests {
    use super::{migrated_to, TestContext};
    use crate::builder::TestContextBuilder;
    use soroban_sdk::testutils::{Address as _, Events as _};
    use soroban_sdk::xdr::ScAddress;
    use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};
    use soroban_testkit_core::storage::StorageSnapshot;

    #[test]
    fn user_accessor_returns_same_as_indexing_users() {
        let ctx = TestContextBuilder::new().with_users(3).build();
        for i in 0..3 {
            assert_eq!(ctx.user(i), &ctx.users[i]);
        }
    }

    #[test]
    #[should_panic(
        expected = "user index out of bounds: index is 2 but context holds 2 users (did you call TestContextBuilder::with_users(3)?)"
    )]
    fn user_accessor_panics_with_diagnostic_when_out_of_bounds() {
        let ctx = TestContextBuilder::new().with_users(2).build();
        let _ = ctx.user(2);
    }

    #[test]
    fn user_accessor_works_after_reset_and_reset_full() {
        let mut ctx = TestContextBuilder::new().with_users(2).build();
        assert_eq!(ctx.user(0), &ctx.users[0]);
        ctx.reset();
        assert_eq!(ctx.user(0), &ctx.users[0]);
        ctx.reset_full();
        assert_eq!(ctx.user(0), &ctx.users[0]);
    }

    #[test]
    fn new_context_starts_without_users() {
        let ctx = TestContext::new();
        assert!(ctx.users.is_empty());
    }

    #[test]
    fn add_user_appends_and_returns_the_new_address() {
        let mut ctx = TestContext::new();

        let first = ctx.add_user().clone();
        let second = ctx.add_user().clone();

        assert_eq!(ctx.users.len(), 2);
        assert_eq!(ctx.users[0], first);
        assert_eq!(ctx.users[1], second);
        assert_ne!(first, second);
    }

    #[test]
    fn admin_is_distinct_from_generated_users() {
        let mut ctx = TestContext::new();
        let user = ctx.add_user().clone();

        assert_ne!(ctx.admin, user);
    }

    #[test]
    fn addresses_generated_in_the_same_env_are_unique() {
        let ctx = TestContext::new();
        let a = Address::generate(&ctx.env);
        let b = Address::generate(&ctx.env);

        assert_ne!(a, b);
    }

    #[test]
    fn default_and_new_build_the_same_shape_of_context() {
        let mut from_default = TestContext::default();
        let mut from_new = TestContext::new();

        assert!(from_default.users.is_empty());
        assert!(from_new.users.is_empty());

        from_default.add_user();
        from_new.add_user();

        assert_eq!(from_default.users.len(), from_new.users.len());
    }

    #[contract]
    pub struct Clock;

    #[contractimpl]
    impl Clock {
        pub fn now(env: Env) -> u64 {
            env.ledger().timestamp()
        }

        pub fn height(env: Env) -> u32 {
            env.ledger().sequence()
        }
    }

    fn clock(env: &Env) -> ClockClient<'_> {
        let id = env.register(Clock, ());
        ClockClient::new(env, &id)
    }

    #[test]
    fn getters_read_the_ledger_the_env_started_with() {
        let ctx = TestContext::new();

        assert_eq!(ctx.timestamp(), ctx.env.ledger().timestamp());
        assert_eq!(ctx.sequence(), ctx.env.ledger().sequence());
    }

    #[test]
    fn a_fresh_env_opens_at_the_zero_ledger_so_set_before_advance() {
        let mut ctx = TestContext::new();
        assert_eq!(ctx.timestamp(), 0);
        assert_eq!(ctx.sequence(), 0);

        // `advance_time` from a zero clock lands on the offset, not on wall
        // time, so a test that cares about an absolute moment sets it first.
        ctx.set_timestamp(1_700_000_000);
        assert_eq!(ctx.advance_time(1), 1_700_000_001);
    }

    #[test]
    fn advance_time_adds_to_the_clock_rather_than_replacing_it() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);

        let after_a_day = ctx.advance_time(86_400);
        let after_two_days = ctx.advance_time(86_400);

        assert_eq!(after_a_day, 1_700_086_400);
        assert_eq!(after_two_days, 1_700_172_800);
        assert_eq!(ctx.timestamp(), after_two_days);
    }

    #[test]
    fn a_contract_sees_the_advanced_clock() {
        let mut ctx = TestContext::new();
        let env = ctx.env.clone();
        let client = clock(&env);
        let before = client.now();

        ctx.advance_time(3_600);

        assert_eq!(client.now(), before + 3_600);
    }

    #[test]
    fn advance_time_leaves_the_sequence_where_it_was() {
        let mut ctx = TestContext::new();
        let height = ctx.sequence();

        ctx.advance_time(86_400 * 30);

        assert_eq!(ctx.sequence(), height);
    }

    #[test]
    fn advance_ledger_moves_the_height_and_not_the_clock() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);
        let height = ctx.sequence();

        let new_height = ctx.advance_ledger(10);

        assert_eq!(new_height, height + 10);
        assert_eq!(ctx.sequence(), new_height);
        assert_eq!(ctx.timestamp(), 1_700_000_000);
    }

    #[test]
    fn advancing_the_clock_and_the_ledger_together_moves_both() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);
        let height = ctx.sequence();

        ctx.advance_time(5 * 10);
        ctx.advance_ledger(10);

        assert_eq!(ctx.timestamp(), 1_700_000_050);
        assert_eq!(ctx.sequence(), height + 10);
    }

    #[test]
    fn a_contract_sees_the_advanced_height() {
        let mut ctx = TestContext::new();
        let env = ctx.env.clone();
        let client = clock(&env);
        let before = client.height();

        ctx.advance_ledger(5);

        assert_eq!(client.height(), before + 5);
    }

    #[test]
    fn set_timestamp_replaces_the_clock() {
        let mut ctx = TestContext::new();

        ctx.set_timestamp(1_000);
        ctx.set_timestamp(2_000);

        assert_eq!(ctx.timestamp(), 2_000);
    }

    #[test]
    fn advancing_past_the_end_of_the_u64_clock_saturates() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(u64::MAX);

        assert_eq!(ctx.advance_time(1), u64::MAX);
    }

    #[test]
    fn advancing_past_the_top_of_u32_saturates_the_height() {
        let mut ctx = TestContext::new();
        let near_top = ctx.advance_ledger(u32::MAX - ctx.sequence());

        assert_eq!(near_top, u32::MAX);
        assert_eq!(ctx.advance_ledger(10), u32::MAX);
    }

    #[contract]
    pub struct Trace;

    #[contractimpl]
    impl Trace {
        #[allow(deprecated)]
        pub fn record(env: Env, caller: Address) {
            caller.require_auth();
            env.storage()
                .persistent()
                .set(&symbol_short!("seen"), &1u32);
            env.events().publish((symbol_short!("recorded"),), 1u32);
        }
    }

    fn trace(env: &Env) -> (Address, TraceClient<'_>) {
        let id = env.register(Trace, ());
        (id.clone(), TraceClient::new(env, &id))
    }

    #[test]
    fn reset_keeps_every_identity_the_context_was_built_with() {
        let mut ctx = TestContextBuilder::new().with_users(3).build();
        let before = identity(&ctx);

        ctx.reset();

        assert_eq!(identity(&ctx), before);
        assert_eq!(ctx.users.len(), 3);
    }

    #[test]
    fn reset_returns_the_ledger_to_where_a_fresh_env_starts() {
        let mut ctx = TestContext::new();
        ctx.set_timestamp(1_700_000_000);
        ctx.advance_ledger(120);

        ctx.reset();

        assert_eq!(ctx.timestamp(), 0);
        assert_eq!(ctx.sequence(), 0);
    }

    #[test]
    fn reset_clears_events_the_previous_env_recorded() {
        let mut ctx = TestContextBuilder::new().with_users(1).build();
        let env = ctx.env.clone();
        let (_id, client) = trace(&env);
        client.record(&ctx.admin);
        assert!(!env.events().all().events().is_empty());

        ctx.reset();

        assert!(ctx.env.events().all().events().is_empty());
    }

    #[test]
    fn reset_leaves_no_trace_of_a_contract_registered_before_it() {
        let mut ctx = TestContextBuilder::new().build();
        let env = ctx.env.clone();
        let (id, client) = trace(&env);
        client.record(&ctx.admin);
        assert!(!StorageSnapshot::capture(&env, &id).is_empty());

        ctx.reset();

        let carried = migrated_to(&ctx.env, &ScAddress::from(&id));
        let after = StorageSnapshot::capture(&ctx.env, &carried);
        assert!(
            after.is_empty(),
            "the reset env still holds {} entries from before it",
            after.len()
        );
    }

    #[test]
    fn reset_reuses_the_mocking_the_context_was_built_with() {
        let mut ctx = TestContextBuilder::new().with_users(1).build();
        ctx.reset();
        let env = ctx.env.clone();
        let (_id, client) = trace(&env);

        client.record(&ctx.users[0]);
    }

    #[test]
    #[should_panic(expected = "Error(Auth, InvalidAction)")]
    fn an_unmocked_context_stays_unmocked_through_reset() {
        let mut ctx = TestContextBuilder::new()
            .with_users(1)
            .without_mock_auths()
            .build();

        ctx.reset();

        let env = ctx.env.clone();
        let (_id, client) = trace(&env);
        client.record(&ctx.users[0]);
    }

    #[test]
    fn the_carried_addresses_still_authorize_calls_in_the_new_env() {
        let mut ctx = TestContextBuilder::new().with_users(1).build();
        ctx.reset();
        let env = ctx.env.clone();
        let (_id, client) = trace(&env);

        client.record(&ctx.users[0]);

        assert_eq!(ctx.env.events().all().events().len(), 1);
    }

    #[test]
    fn reset_full_replaces_every_identity() {
        let mut ctx = TestContextBuilder::new().with_users(3).build();
        let before = identity(&ctx);

        ctx.reset_full();

        let after = identity(&ctx);
        assert_eq!(after.len(), before.len());
        for carried in &before {
            assert!(
                !after.contains(carried),
                "{carried:?} survived a reset_full"
            );
        }
    }

    #[test]
    fn regenerating_without_the_counter_walk_would_reuse_the_same_addresses() {
        let ctx = TestContextBuilder::new().with_users(3).build();
        let before = identity(&ctx);

        // The reason `reset_full` burns generator slots: a fresh env mints
        // addresses from the same counter, so a naive rebuild is not a rebuild.
        let env = Env::default();
        let naive = std::iter::once(Address::generate(&env))
            .chain((0..3).map(|_| Address::generate(&env)))
            .map(ScAddress::from)
            .collect::<Vec<_>>();
        assert_eq!(naive, before);

        let mut reset = TestContextBuilder::new().with_users(3).build();
        reset.reset_full();
        assert_ne!(identity(&reset), before);
    }

    fn identity(ctx: &TestContext) -> Vec<ScAddress> {
        std::iter::once(&ctx.admin)
            .chain(ctx.users.iter())
            .map(ScAddress::from)
            .collect()
    }
}
