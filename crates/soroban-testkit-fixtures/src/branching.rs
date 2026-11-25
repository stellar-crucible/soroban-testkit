//! Named checkpoints over a [`TestContext`], for speculative multi-path testing.
//!
//! A test that explores five trade routes from one pool state should build the
//! pool once, snapshot it, and then run each route against its own copy. A
//! [`BranchingContext`] records the ledger — entries, TTLs and clock — under a
//! name and restores it on demand without re-running any setup code:
//!
//! ```rust
//! use soroban_sdk::{contract, contractimpl, Env};
//! use soroban_testkit_fixtures::branching::BranchingContext;
//! use soroban_testkit_fixtures::builder::TestContextBuilder;
//!
//! #[contract]
//! pub struct Pool;
//!
//! #[contractimpl]
//! impl Pool {
//!     pub fn deposit(env: Env, amount: u32) {
//!         let current: u32 = env.storage().instance().get(&"total").unwrap_or(0);
//!         env.storage().instance().set(&"total", &(current + amount));
//!     }
//!
//!     pub fn total(env: Env) -> u32 {
//!         env.storage().instance().get(&"total").unwrap_or(0)
//!     }
//! }
//!
//! # fn main() {
//! let mut ctx = BranchingContext::new(TestContextBuilder::new().with_users(2).build());
//! let pool_id = ctx.env.register(Pool, ());
//!
//! // Setup, executed once.
//! PoolClient::new(&ctx.env, &pool_id).deposit(&1_000);
//! ctx.create_branch("initial_pool");
//!
//! // Route A.
//! PoolClient::new(&ctx.env, &pool_id).deposit(&250);
//! ctx.create_branch("route_a");
//!
//! // Route B starts from the same pool state, not from route A.
//! ctx.switch_branch("initial_pool");
//! assert_eq!(PoolClient::new(&ctx.env, &pool_id).total(), 1_000);
//! # }
//! ```
//!
//! # What is restored
//!
//! Switching rewrites the live environment in place, so it restores:
//!
//! - every captured ledger entry — contract code, instances, persistent and
//!   temporary storage — with its value and `live_until` TTL;
//! - contract data entries added after the checkpoint, whose keys are removed;
//! - the ledger clock: sequence, timestamp, protocol version, network id, base
//!   reserve and the TTL thresholds.
//!
//! Because the [`Env`] itself is kept, native test contracts stay registered and
//! the context's auth policy (`mock_all_auths`) and network profile keep
//! applying. Addresses, clients and contract ids from before a switch remain
//! valid; they all point at the same environment.
//!
//! Two things a switch cannot undo, because the SDK exposes no way to delete
//! them:
//!
//! - a contract registered **after** the checkpoint keeps its code and instance
//!   entries (its storage is cleared, but the contract remains callable);
//! - authorization history recorded by `env.auths()` belongs to the most recent
//!   invocation and is not part of a checkpoint.
//!
//! Register every contract the test needs before taking a checkpoint, and assert
//! authorizations immediately after the call that produced them.

use std::collections::BTreeMap;

use soroban_sdk::testutils::{Ledger as _, Snapshot};
use soroban_sdk::xdr::{ContractDataDurability, LedgerEntry, LedgerEntryData, LedgerKey, ScVal};
use soroban_sdk::{Address, Env, TryFromVal as _, Val};

use crate::TestContext;

/// A [`TestContext`] with named checkpoints that can be restored non-destructively.
///
/// The context itself is available through `Deref`/`DerefMut`, so `ctx.env`,
/// `ctx.users` and every helper method on [`TestContext`] keep working while the
/// branch API lives on the wrapper.
pub struct BranchingContext {
    context: TestContext,
    branches: BTreeMap<String, Snapshot>,
    current: Option<String>,
}

impl BranchingContext {
    /// Wraps an existing context. It starts with no branches recorded.
    pub fn new(context: TestContext) -> Self {
        Self {
            context,
            branches: BTreeMap::new(),
            current: None,
        }
    }

    /// Records the current ledger under `name` and makes it the current branch.
    ///
    /// The snapshot is taken immediately, so later calls that mutate storage or
    /// advance the ledger do not change what this branch restores.
    ///
    /// # Panics
    ///
    /// Panics when a branch with this name already exists. Delete it with
    /// [`Self::delete_branch`] first, or choose another name.
    pub fn create_branch(&mut self, name: impl Into<String>) -> &mut Self {
        let name = name.into();
        assert!(
            !self.branches.contains_key(&name),
            "branch \"{name}\" already exists; branches: {:?}",
            self.branch_names()
        );
        self.branches
            .insert(name.clone(), self.context.env.to_snapshot());
        self.current = Some(name);
        self
    }

    /// Restores the ledger recorded under `name`.
    ///
    /// Storage values, TTLs, contract data added later, and the ledger clock all
    /// return to the moment [`Self::create_branch`] captured them. The next
    /// branch created snapshots this restored state, not the state that was
    /// current before the switch.
    ///
    /// # Panics
    ///
    /// Panics when no branch named `name` exists, listing the recorded branches.
    pub fn switch_branch(&mut self, name: &str) -> &mut Self {
        let snapshot = self
            .branches
            .get(name)
            .unwrap_or_else(|| {
                panic!(
                    "no branch named \"{name}\"; recorded branches: {:?}",
                    self.branch_names()
                )
            })
            .clone();
        restore(&self.context.env, &snapshot);
        self.current = Some(name.to_string());
        self
    }

    /// The branch last created or switched to.
    pub fn current_branch(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Every recorded branch name, in sorted order.
    pub fn branch_names(&self) -> Vec<&str> {
        self.branches.keys().map(String::as_str).collect()
    }

    /// Whether a branch named `name` has been recorded.
    pub fn has_branch(&self, name: &str) -> bool {
        self.branches.contains_key(name)
    }

    /// Drops the checkpoint named `name`, returning whether it existed.
    ///
    /// Dropping the current branch leaves the ledger exactly as it is; only the
    /// ability to restore it is lost.
    pub fn delete_branch(&mut self, name: &str) -> bool {
        let removed = self.branches.remove(name).is_some();
        if removed && self.current.as_deref() == Some(name) {
            self.current = None;
        }
        removed
    }

    /// The wrapped context, for branch-independent fixture setup.
    pub fn context(&self) -> &TestContext {
        &self.context
    }

    /// The wrapped context, mutable.
    pub fn context_mut(&mut self) -> &mut TestContext {
        &mut self.context
    }

    /// Consumes the wrapper and returns the context at its current state.
    pub fn into_context(self) -> TestContext {
        self.context
    }
}

impl std::ops::Deref for BranchingContext {
    type Target = TestContext;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

impl std::ops::DerefMut for BranchingContext {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.context
    }
}

/// Rewrites `env`'s ledger to match `snapshot`.
fn restore(env: &Env, snapshot: &Snapshot) {
    let ledger = &snapshot.ledger;

    // Delete contract data the checkpoint does not hold. Removals go through the
    // contract storage API because the host has no public ledger-entry delete.
    let wanted: Vec<&LedgerKey> = ledger
        .entries()
        .into_iter()
        .map(|(key, _)| &**key)
        .collect();
    let current = env.to_ledger_snapshot();
    for (key, (entry, _)) in current.entries() {
        if wanted.contains(&&**key) {
            continue;
        }
        remove_contract_data(env, key, entry);
    }

    // Rewrite every captured entry with its value and TTL. This also restores
    // entries the test contract deleted or overwrote since the checkpoint.
    for (key, (entry, live_until)) in ledger.entries().into_iter() {
        env.host()
            .add_ledger_entry(
                &std::rc::Rc::new((**key).clone()),
                &std::rc::Rc::new((**entry).clone()),
                *live_until,
            )
            .expect("the host accepts the checkpointed ledger entry");
    }

    env.ledger().set(ledger.ledger_info());
}

/// Remove one live contract data entry, whether it is a per-key entry or the
/// instance container holding the whole instance storage map.
fn remove_contract_data(env: &Env, key: &LedgerKey, entry: &LedgerEntry) {
    let LedgerKey::ContractData(data) = key else {
        return;
    };
    let Ok(contract) = Address::try_from_val(env, &data.contract) else {
        return;
    };

    if data.key != ScVal::LedgerKeyContractInstance {
        let Ok(stored_key) = Val::try_from_val(env, &data.key) else {
            return;
        };
        env.as_contract(&contract, || match data.durability {
            ContractDataDurability::Persistent => env.storage().persistent().remove(&stored_key),
            ContractDataDurability::Temporary => env.storage().temporary().remove(&stored_key),
        });
        return;
    }

    // Instance storage lives inside the instance container's value, so the keys
    // to remove are read from the entry that is going away.
    let LedgerEntryData::ContractData(stored) = &entry.data else {
        return;
    };
    let ScVal::ContractInstance(instance) = &stored.val else {
        return;
    };
    let Some(storage) = &instance.storage else {
        return;
    };
    let keys: Vec<Val> = storage
        .0
        .iter()
        .filter_map(|item| Val::try_from_val(env, &item.key).ok())
        .collect();
    env.as_contract(&contract, || {
        for key in keys {
            env.storage().instance().remove(&key);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::BranchingContext;
    use crate::builder::TestContextBuilder;
    use soroban_sdk::{contract, contractimpl, symbol_short, Env};

    #[contract]
    pub struct Pool;

    #[contractimpl]
    impl Pool {
        pub fn deposit(env: Env, amount: u32) {
            let current: u32 = env.storage().instance().get(&"total").unwrap_or(0);
            env.storage().instance().set(&"total", &(current + amount));
        }

        pub fn total(env: Env) -> u32 {
            env.storage().instance().get(&"total").unwrap_or(0)
        }

        pub fn remember(env: Env, value: u32) {
            env.storage()
                .persistent()
                .set(&symbol_short!("memo"), &value);
        }

        pub fn memo(env: Env) -> Option<u32> {
            env.storage().persistent().get(&symbol_short!("memo"))
        }

        pub fn forget(env: Env) {
            env.storage().persistent().remove(&symbol_short!("memo"));
        }
    }

    fn context() -> BranchingContext {
        BranchingContext::new(TestContextBuilder::new().with_users(2).build())
    }

    #[test]
    fn a_branch_restores_storage_and_the_ledger_clock() {
        let mut ctx = context();
        let pool_id = ctx.env.register(Pool, ());
        let pool = PoolClient::new(&ctx.env, &pool_id);

        pool.deposit(&1_000);
        ctx.create_branch("initial_pool");

        pool.deposit(&250);
        ctx.advance_ledger(10);
        ctx.advance_time(50);
        ctx.create_branch("after_deposit");
        assert_eq!(pool.total(), 1_250);

        ctx.switch_branch("initial_pool");

        assert_eq!(pool.total(), 1_000);
        assert_eq!(ctx.sequence(), 0);
        assert_eq!(ctx.timestamp(), 0);
    }

    #[test]
    fn switching_back_to_a_later_branch_restores_it_too() {
        let mut ctx = context();
        let pool_id = ctx.env.register(Pool, ());
        let pool = PoolClient::new(&ctx.env, &pool_id);

        pool.deposit(&1_000);
        ctx.create_branch("initial_pool");
        pool.deposit(&250);
        pool.remember(&7);
        ctx.advance_ledger(10);
        ctx.advance_time(50);
        ctx.create_branch("after_deposit");

        ctx.switch_branch("initial_pool");
        assert_eq!(pool.memo(), None);

        ctx.switch_branch("after_deposit");
        assert_eq!(pool.total(), 1_250);
        assert_eq!(pool.memo(), Some(7));
        assert_eq!(ctx.sequence(), 10);
        assert_eq!(ctx.timestamp(), 50);
    }

    #[test]
    fn a_branch_restores_an_entry_the_contract_deleted() {
        let mut ctx = context();
        let pool_id = ctx.env.register(Pool, ());
        let pool = PoolClient::new(&ctx.env, &pool_id);

        pool.remember(&7);
        ctx.create_branch("with_memo");
        pool.forget();
        assert_eq!(pool.memo(), None);

        ctx.switch_branch("with_memo");

        assert_eq!(pool.memo(), Some(7));
    }

    #[test]
    fn a_switch_removes_keys_written_after_the_checkpoint() {
        let mut ctx = context();
        let pool_id = ctx.env.register(Pool, ());
        let pool = PoolClient::new(&ctx.env, &pool_id);

        pool.deposit(&1_000);
        ctx.create_branch("initial_pool");
        pool.remember(&7);
        assert_eq!(pool.memo(), Some(7));

        ctx.switch_branch("initial_pool");

        assert_eq!(pool.memo(), None);
        assert_eq!(pool.total(), 1_000);
    }

    #[test]
    fn branches_share_the_setup_without_re_executing_it() {
        let mut ctx = context();
        let pool_id = ctx.env.register(Pool, ());
        let pool = PoolClient::new(&ctx.env, &pool_id);

        // The one and only setup call.
        pool.deposit(&1_000);
        ctx.create_branch("initial_pool");

        for route in 0..5u32 {
            ctx.switch_branch("initial_pool")
                .create_branch(format!("route_{route}"));
            pool.deposit(&((route + 1) * 100));
            assert_eq!(pool.total(), 1_000 + (route + 1) * 100);
        }

        assert_eq!(
            ctx.branch_names(),
            vec![
                "initial_pool",
                "route_0",
                "route_1",
                "route_2",
                "route_3",
                "route_4"
            ]
        );
    }

    #[test]
    fn clients_and_addresses_stay_valid_across_a_switch() {
        let mut ctx = context();
        let pool_id = ctx.env.register(Pool, ());
        let pool = PoolClient::new(&ctx.env, &pool_id);
        let admin = ctx.admin.clone();
        let user = ctx.users[0].clone();

        pool.deposit(&1_000);
        ctx.create_branch("initial");
        ctx.switch_branch("initial");

        assert_eq!(ctx.admin, admin);
        assert_eq!(ctx.users[0], user);
        assert_eq!(ctx.users.len(), 2);
        pool.deposit(&1);
        assert_eq!(pool.total(), 1_001);
    }

    #[test]
    fn an_unmocked_context_stays_unmocked_after_a_switch() {
        let mut ctx = BranchingContext::new(
            TestContextBuilder::new()
                .with_users(1)
                .without_mock_auths()
                .build(),
        );
        ctx.create_branch("initial");
        ctx.switch_branch("initial");

        // `audit` requires auth; with mocking off this panics with the auth
        // error rather than recording a requirement.
        let id = ctx.env.register(Auth, ());
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            AuthClient::new(&ctx.env, &id).audit(&ctx.users[0]);
        }));
        assert!(
            outcome.is_err(),
            "auth must still be enforced after a switch"
        );
    }

    #[contract]
    pub struct Auth;

    #[contractimpl]
    impl Auth {
        pub fn audit(env: Env, signer: soroban_sdk::Address) {
            signer.require_auth();
            let _ = env;
        }
    }

    #[test]
    fn the_current_branch_tracks_creation_and_switching() {
        let mut ctx = context();
        assert_eq!(ctx.current_branch(), None);

        ctx.create_branch("one");
        assert_eq!(ctx.current_branch(), Some("one"));

        ctx.advance_ledger(1);
        ctx.create_branch("two");
        assert_eq!(ctx.current_branch(), Some("two"));

        ctx.switch_branch("one");
        assert_eq!(ctx.current_branch(), Some("one"));
    }

    #[test]
    #[should_panic(expected = "branch \"one\" already exists")]
    fn creating_a_duplicate_branch_is_refused() {
        let mut ctx = context();
        ctx.create_branch("one");
        ctx.create_branch("one");
    }

    #[test]
    #[should_panic(expected = "no branch named \"missing\"; recorded branches: [\"one\"]")]
    fn switching_to_an_unknown_branch_lists_the_recorded_ones() {
        let mut ctx = context();
        ctx.create_branch("one");
        ctx.switch_branch("missing");
    }

    #[test]
    fn deleting_a_branch_drops_only_its_checkpoint() {
        let mut ctx = context();
        ctx.create_branch("one");
        assert!(ctx.delete_branch("one"));
        assert!(!ctx.delete_branch("one"));
        assert_eq!(ctx.current_branch(), None);
        assert!(ctx.branch_names().is_empty());
    }
}
