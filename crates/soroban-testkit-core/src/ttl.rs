//! Reading the live TTL and rent thresholds of a contract storage entry.
//!
//! Soroban gives every contract-data entry a `live_until_ledger`: the last
//! ledger sequence at which the entry can still be read. Past it, a temporary
//! entry is gone for good, and a persistent entry (or a contract instance) is
//! archived until something restores it. Testing those boundaries by hand means
//! arithmetic on sequence numbers that silently goes stale when the network's
//! rent parameters change.
//!
//! [`TtlInspector`] reads the TTL straight out of the host's ledger storage —
//! without touching it, so inspecting an expired entry does not trigger the
//! host's evict-or-restore handling — and pairs it with the rent parameters the
//! ledger is configured with:
//!
//! ```rust
//! use soroban_sdk::{contract, contractimpl, symbol_short, Env};
//! use soroban_testkit_core::ttl::{TtlInspector, TtlStatus};
//! use soroban_testkit_core::StorageTier;
//!
//! #[contract]
//! pub struct Nonces;
//!
//! #[contractimpl]
//! impl Nonces {
//!     pub fn put(env: Env) {
//!         env.storage().temporary().set(&symbol_short!("nonce"), &1u32);
//!     }
//! }
//!
//! # fn main() {
//! let env = Env::default();
//! let id = env.register(Nonces, ());
//! NoncesClient::new(&env, &id).put();
//!
//! let ttl = TtlInspector::for_key(&env, &id, &symbol_short!("nonce"));
//! assert_eq!(ttl.tier, StorageTier::Temporary);
//! assert!(ttl.live_until_ledger >= ttl.current_ledger);
//! assert_eq!(ttl.status(), TtlStatus::Live);
//! # }
//! ```

use std::fmt;
use std::rc::Rc;

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::xdr::{
    ContractDataDurability, LedgerEntryData, LedgerKey, LedgerKeyContractData, ScAddress, ScVal,
};
use soroban_sdk::{Address, Env, IntoVal, TryFromVal, Val};

use crate::storage::StorageTier;

/// Where an entry stands relative to its TTL at the inspected ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TtlStatus {
    /// The entry can be read at the current ledger.
    Live,
    /// A temporary entry past its TTL: the next access finds nothing, and no
    /// restore can bring it back.
    Expired,
    /// A persistent entry (or contract instance) past its TTL: archived, and
    /// readable again only after a restore.
    Archived,
}

impl TtlStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TtlStatus::Live => "live",
            TtlStatus::Expired => "expired",
            TtlStatus::Archived => "archived",
        }
    }
}

impl fmt::Display for TtlStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The rent parameters of the ledger an entry was inspected on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RentThresholds {
    /// TTL a new temporary entry starts with, in ledgers counting the current one.
    pub min_temp_entry_ttl: u32,
    /// TTL a new persistent entry (and instance) starts with.
    pub min_persistent_entry_ttl: u32,
    /// The furthest any entry can live, counting the current ledger.
    pub max_entry_ttl: u32,
}

impl RentThresholds {
    /// Read the thresholds the environment's ledger is configured with.
    pub fn of(env: &Env) -> Self {
        let info = env.ledger().get();
        Self {
            min_temp_entry_ttl: info.min_temp_entry_ttl,
            min_persistent_entry_ttl: info.min_persistent_entry_ttl,
            max_entry_ttl: info.max_entry_ttl,
        }
    }

    /// The TTL a freshly written entry of `tier` is given.
    pub fn min_ttl_for(&self, tier: StorageTier) -> u32 {
        match tier {
            StorageTier::Temporary => self.min_temp_entry_ttl,
            StorageTier::Persistent | StorageTier::Instance => self.min_persistent_entry_ttl,
        }
    }
}

/// The TTL of one storage entry, read at one ledger.
///
/// This is a snapshot: advancing the ledger or extending the entry does not
/// change it. Call [`TtlInspector::refresh`] to read it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtlInspector {
    /// The contract that owns the entry.
    pub contract: Address,
    /// The key as the host stores it. For [`StorageTier::Instance`] this is the
    /// key inside the instance map; the TTL is the instance entry's.
    pub key: ScVal,
    /// Which storage the entry lives in.
    pub tier: StorageTier,
    /// The last ledger the entry is readable at.
    pub live_until_ledger: u32,
    /// The ledger sequence the inspection ran at.
    pub current_ledger: u32,
    /// The rent parameters of the ledger at inspection time.
    pub thresholds: RentThresholds,
}

impl TtlInspector {
    /// Inspect the entry `contract` stores under `key`, in whichever tier holds
    /// it.
    ///
    /// Persistent storage is searched first, then temporary, then the instance
    /// map. A contract that uses the same key in two tiers should name the tier
    /// with [`TtlInspector::persistent`], [`TtlInspector::temporary`] or
    /// [`TtlInspector::instance`] instead.
    ///
    /// # Panics
    ///
    /// Panics when no tier holds `key`, naming the contract and the key.
    pub fn for_key<K: IntoVal<Env, Val>>(env: &Env, contract: &Address, key: &K) -> Self {
        let key = sc_key(env, key);
        [
            StorageTier::Persistent,
            StorageTier::Temporary,
            StorageTier::Instance,
        ]
        .into_iter()
        .find_map(|tier| Self::read(env, contract, &key, tier))
        .unwrap_or_else(|| {
            panic!(
                "TtlInspector: contract {contract:?} holds no entry for key {key:?} in persistent, temporary or instance storage"
            )
        })
    }

    /// Inspect `key`, returning `None` instead of panicking when no tier holds it.
    pub fn try_for_key<K: IntoVal<Env, Val>>(
        env: &Env,
        contract: &Address,
        key: &K,
    ) -> Option<Self> {
        let key = sc_key(env, key);
        [
            StorageTier::Persistent,
            StorageTier::Temporary,
            StorageTier::Instance,
        ]
        .into_iter()
        .find_map(|tier| Self::read(env, contract, &key, tier))
    }

    /// Inspect `key` in persistent storage.
    ///
    /// # Panics
    ///
    /// Panics when persistent storage does not hold `key`.
    pub fn persistent<K: IntoVal<Env, Val>>(env: &Env, contract: &Address, key: &K) -> Self {
        Self::in_tier(env, contract, key, StorageTier::Persistent)
    }

    /// Inspect `key` in temporary storage.
    ///
    /// # Panics
    ///
    /// Panics when temporary storage does not hold `key`. An expired temporary
    /// entry the host has already dropped on access is no longer held.
    pub fn temporary<K: IntoVal<Env, Val>>(env: &Env, contract: &Address, key: &K) -> Self {
        Self::in_tier(env, contract, key, StorageTier::Temporary)
    }

    /// Inspect the contract instance itself: the TTL every instance-storage key
    /// shares.
    ///
    /// # Panics
    ///
    /// Panics when `contract` is not a contract registered in `env`.
    pub fn instance(env: &Env, contract: &Address) -> Self {
        let (_, live_until) = instance_entry(env, contract).unwrap_or_else(|| {
            panic!("TtlInspector: {contract:?} has no contract instance entry in this environment")
        });
        Self::new(
            env,
            contract,
            ScVal::LedgerKeyContractInstance,
            StorageTier::Instance,
            live_until,
        )
    }

    /// Inspect `key` in the given tier, or `None` when that tier does not hold it.
    pub fn try_in_tier<K: IntoVal<Env, Val>>(
        env: &Env,
        contract: &Address,
        key: &K,
        tier: StorageTier,
    ) -> Option<Self> {
        Self::read(env, contract, &sc_key(env, key), tier)
    }

    fn in_tier<K: IntoVal<Env, Val>>(
        env: &Env,
        contract: &Address,
        key: &K,
        tier: StorageTier,
    ) -> Self {
        let key = sc_key(env, key);
        Self::read(env, contract, &key, tier).unwrap_or_else(|| {
            panic!(
                "TtlInspector: contract {contract:?} holds no {} entry for key {key:?}",
                tier.as_str()
            )
        })
    }

    fn read(env: &Env, contract: &Address, key: &ScVal, tier: StorageTier) -> Option<Self> {
        let live_until = match tier {
            StorageTier::Persistent => {
                data_entry_live_until(env, contract, key, ContractDataDurability::Persistent)?
            }
            StorageTier::Temporary => {
                data_entry_live_until(env, contract, key, ContractDataDurability::Temporary)?
            }
            StorageTier::Instance => {
                let (instance_keys, live_until) = instance_entry(env, contract)?;
                if !instance_keys.iter().any(|stored| stored == key) {
                    return None;
                }
                live_until
            }
        };
        Some(Self::new(env, contract, key.clone(), tier, live_until))
    }

    fn new(env: &Env, contract: &Address, key: ScVal, tier: StorageTier, live_until: u32) -> Self {
        Self {
            contract: contract.clone(),
            key,
            tier,
            live_until_ledger: live_until,
            current_ledger: env.ledger().sequence(),
            thresholds: RentThresholds::of(env),
        }
    }

    /// Read the same entry again at the environment's current ledger.
    ///
    /// Returns `None` when the entry is gone — a temporary entry the host
    /// dropped, or a persistent one a sweep archived.
    pub fn refresh(&self, env: &Env) -> Option<Self> {
        if self.tier == StorageTier::Instance && self.key == ScVal::LedgerKeyContractInstance {
            let (_, live_until) = instance_entry(env, &self.contract)?;
            return Some(Self::new(
                env,
                &self.contract,
                self.key.clone(),
                self.tier,
                live_until,
            ));
        }
        Self::read(env, &self.contract, &self.key, self.tier)
    }

    /// Ledgers left after the current one, in the units of the SDK's `get_ttl`
    /// and `extend_ttl`: `live_until_ledger - current_ledger`, or `0` once the
    /// entry has expired.
    pub fn ttl(&self) -> u32 {
        self.live_until_ledger.saturating_sub(self.current_ledger)
    }

    /// Whether the entry can be read at the inspected ledger.
    pub fn is_live(&self) -> bool {
        self.live_until_ledger >= self.current_ledger
    }

    /// Whether the entry's TTL has run out.
    pub fn is_expired(&self) -> bool {
        !self.is_live()
    }

    /// What an access at the inspected ledger would find.
    pub fn status(&self) -> TtlStatus {
        match (self.is_live(), self.tier) {
            (true, _) => TtlStatus::Live,
            (false, StorageTier::Temporary) => TtlStatus::Expired,
            (false, _) => TtlStatus::Archived,
        }
    }

    /// The first ledger at which the entry is no longer live.
    pub fn expiration_ledger(&self) -> u32 {
        self.live_until_ledger.saturating_add(1)
    }

    /// Ledgers the clock has to move for the entry to expire.
    pub fn ledgers_until_expiration(&self) -> u32 {
        self.expiration_ledger().saturating_sub(self.current_ledger)
    }

    /// The TTL a freshly written entry of this tier would be given.
    pub fn min_ttl(&self) -> u32 {
        self.thresholds.min_ttl_for(self.tier)
    }

    /// The furthest `live_until_ledger` the host allows from the inspected
    /// ledger: `current + max_entry_ttl - 1`.
    pub fn max_live_until_ledger(&self) -> u32 {
        self.current_ledger
            .saturating_add(self.thresholds.max_entry_ttl.saturating_sub(1))
    }

    /// The largest `extend_to` that still moves the entry, i.e. the TTL it
    /// would have if extended to the network ceiling.
    pub fn max_extend_to(&self) -> u32 {
        self.max_live_until_ledger()
            .saturating_sub(self.current_ledger)
    }

    /// Whether `extend_ttl(key, threshold, _)` would fire at this ledger.
    ///
    /// The host extends when the current TTL is at or below `threshold`.
    pub fn needs_extension(&self, threshold: u32) -> bool {
        self.ttl() <= threshold
    }

    /// The `live_until_ledger` `extend_ttl(key, threshold, extend_to)` would
    /// leave the entry with, or `None` when the call would not change it.
    ///
    /// Mirrors the host: the call fires only when the TTL is at or below
    /// `threshold`, the new `live_until` is `current + extend_to` clamped to
    /// [`TtlInspector::max_live_until_ledger`], and a TTL is never shortened.
    pub fn predict_extend_ttl(&self, threshold: u32, extend_to: u32) -> Option<u32> {
        if !self.is_live() || !self.needs_extension(threshold) {
            return None;
        }
        let target = self
            .current_ledger
            .saturating_add(extend_to)
            .min(self.max_live_until_ledger());
        (target > self.live_until_ledger).then_some(target)
    }

    /// Fail unless the entry is live at the inspected ledger.
    #[track_caller]
    pub fn assert_live(&self) {
        assert!(
            self.is_live(),
            "Expected {} to be live at ledger {}, but it expired after ledger {} ({} status)",
            self.describe(),
            self.current_ledger,
            self.live_until_ledger,
            self.status()
        );
    }

    /// Fail unless the entry's TTL has run out at the inspected ledger.
    #[track_caller]
    pub fn assert_expired(&self) {
        assert!(
            self.is_expired(),
            "Expected {} to have expired at ledger {}, but it is live until ledger {} ({} ledgers left)",
            self.describe(),
            self.current_ledger,
            self.live_until_ledger,
            self.ttl()
        );
    }

    /// Fail unless at least `ledgers` remain on the entry's TTL.
    #[track_caller]
    pub fn assert_ttl_at_least(&self, ledgers: u32) {
        assert!(
            self.ttl() >= ledgers,
            "Expected {} to have at least {ledgers} ledgers of TTL at ledger {}, but it has {} (live until {})",
            self.describe(),
            self.current_ledger,
            self.ttl(),
            self.live_until_ledger
        );
    }

    /// `tier:key of contract`, the form the failure messages quote.
    pub fn describe(&self) -> String {
        format!(
            "{} entry {:?} of {:?}",
            self.tier.as_str(),
            self.key,
            self.contract
        )
    }
}

impl fmt::Display for TtlInspector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: live until {} (ledger {}, ttl {}, {})",
            self.describe(),
            self.live_until_ledger,
            self.current_ledger,
            self.ttl(),
            self.status()
        )
    }
}

/// What a call did to an entry's TTL.
///
/// Built by [`TtlExtension::observe`], which inspects the entry, runs the call,
/// and inspects it again, so a test can check an `extend_ttl` inside a
/// contract against the host's rules instead of re-deriving them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtlExtension {
    pub before: TtlInspector,
    pub after: TtlInspector,
}

impl TtlExtension {
    /// Inspect `key` around `call`.
    ///
    /// # Panics
    ///
    /// Panics when no tier holds `key` before the call, or when the call
    /// removes it.
    pub fn observe<K, F, T>(env: &Env, contract: &Address, key: &K, call: F) -> Self
    where
        K: IntoVal<Env, Val>,
        F: FnOnce() -> T,
    {
        let before = TtlInspector::for_key(env, contract, key);
        Self::observe_from(env, before, call)
    }

    /// Run `call` and read `before`'s entry again afterwards.
    pub fn observe_from<F, T>(env: &Env, before: TtlInspector, call: F) -> Self
    where
        F: FnOnce() -> T,
    {
        let _ = call();
        let after = before.refresh(env).unwrap_or_else(|| {
            panic!("TtlExtension: {} is gone after the call", before.describe())
        });
        Self { before, after }
    }

    /// How many ledgers the call added to `live_until_ledger`.
    pub fn extended_by(&self) -> u32 {
        self.after
            .live_until_ledger
            .saturating_sub(self.before.live_until_ledger)
    }

    /// Whether the call moved `live_until_ledger` at all.
    pub fn was_extended(&self) -> bool {
        self.after.live_until_ledger > self.before.live_until_ledger
    }

    /// Fail unless the entry now has exactly `extend_to` ledgers of TTL,
    /// clamped to the network ceiling the way the host clamps it.
    #[track_caller]
    pub fn assert_extended_to(&self, extend_to: u32) {
        let expected = self
            .after
            .current_ledger
            .saturating_add(extend_to)
            .min(self.after.max_live_until_ledger());
        assert_eq!(
            self.after.live_until_ledger,
            expected,
            "Expected {} to be extended to live until ledger {expected} (extend_to = {extend_to} at ledger {}), but it lives until {} (was {})",
            self.after.describe(),
            self.after.current_ledger,
            self.after.live_until_ledger,
            self.before.live_until_ledger
        );
    }

    /// Fail unless the call left `live_until_ledger` where it was.
    #[track_caller]
    pub fn assert_not_extended(&self) {
        assert!(
            !self.was_extended(),
            "Expected {} not to be extended, but its live_until moved from {} to {} (+{})",
            self.after.describe(),
            self.before.live_until_ledger,
            self.after.live_until_ledger,
            self.extended_by()
        );
    }

    /// Fail unless the call did exactly what `extend_ttl(key, threshold,
    /// extend_to)` does under the host's rules, judged from the `before`
    /// reading: extend when the TTL was at or below `threshold`, leave it
    /// alone otherwise.
    #[track_caller]
    pub fn assert_matches_extend_ttl(&self, threshold: u32, extend_to: u32) {
        let expected = self
            .before
            .predict_extend_ttl(threshold, extend_to)
            .unwrap_or(self.before.live_until_ledger);
        assert_eq!(
            self.after.live_until_ledger,
            expected,
            "extend_ttl(threshold = {threshold}, extend_to = {extend_to}) on {} with ttl {} at ledger {} should leave live_until at {expected}, but it is {}",
            self.before.describe(),
            self.before.ttl(),
            self.before.current_ledger,
            self.after.live_until_ledger
        );
    }
}

fn sc_key<K: IntoVal<Env, Val>>(env: &Env, key: &K) -> ScVal {
    let val: Val = key.into_val(env);
    ScVal::try_from_val(env, &val).unwrap_or_else(|error| {
        panic!("TtlInspector: the key does not convert to an ScVal: {error:?}")
    })
}

/// Find `key` in the host's raw storage map without the expiry handling a
/// normal read performs, so inspecting an expired entry leaves it in place.
fn stored_live_until(
    env: &Env,
    ledger_key: &LedgerKey,
) -> Option<(Rc<soroban_sdk::xdr::LedgerEntry>, u32)> {
    let stored = env
        .host()
        .get_stored_entries()
        .unwrap_or_else(|error| panic!("TtlInspector could not read the ledger: {error:?}"));
    stored
        .into_iter()
        .find(|(key, _)| key.as_ref() == ledger_key)
        .and_then(|(_, entry)| entry)
        .and_then(|(entry, live_until)| live_until.map(|until| (entry, until)))
}

fn data_entry_live_until(
    env: &Env,
    contract: &Address,
    key: &ScVal,
    durability: ContractDataDurability,
) -> Option<u32> {
    let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::from(contract),
        key: key.clone(),
        durability,
    });
    stored_live_until(env, &ledger_key).map(|(_, until)| until)
}

/// The instance map's keys and the instance entry's `live_until`.
fn instance_entry(env: &Env, contract: &Address) -> Option<(Vec<ScVal>, u32)> {
    let ledger_key = LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::from(contract),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    });
    let (entry, live_until) = stored_live_until(env, &ledger_key)?;
    let keys = match &entry.data {
        LedgerEntryData::ContractData(data) => match &data.val {
            ScVal::ContractInstance(instance) => instance
                .storage
                .as_ref()
                .map(|map| map.iter().map(|entry| entry.key.clone()).collect())
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    Some((keys, live_until))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{contract, contractimpl, symbol_short, Symbol};

    const KEY: Symbol = symbol_short!("k");

    #[contract]
    pub struct Tiers;

    #[contractimpl]
    impl Tiers {
        pub fn put_persistent(env: Env, value: u32) {
            env.storage().persistent().set(&KEY, &value);
        }

        pub fn put_temporary(env: Env, value: u32) {
            env.storage().temporary().set(&KEY, &value);
        }

        pub fn put_instance(env: Env, value: u32) {
            env.storage().instance().set(&KEY, &value);
        }

        pub fn get_temporary(env: Env) -> Option<u32> {
            env.storage().temporary().get(&KEY)
        }

        pub fn get_persistent(env: Env) -> Option<u32> {
            env.storage().persistent().get(&KEY)
        }

        pub fn bump_persistent(env: Env, threshold: u32, extend_to: u32) {
            env.storage()
                .persistent()
                .extend_ttl(&KEY, threshold, extend_to);
        }

        pub fn bump_temporary(env: Env, threshold: u32, extend_to: u32) {
            env.storage()
                .temporary()
                .extend_ttl(&KEY, threshold, extend_to);
        }

        pub fn bump_instance(env: Env, threshold: u32, extend_to: u32) {
            env.storage().instance().extend_ttl(threshold, extend_to);
        }
    }

    fn setup() -> (Env, Address, TiersClient<'static>) {
        let env = Env::default();
        let id = env.register(Tiers, ());
        let client = TiersClient::new(&env, &id);
        (env, id, client)
    }

    #[test]
    fn a_new_persistent_entry_carries_the_minimum_persistent_ttl() {
        let (env, id, client) = setup();
        client.put_persistent(&1);

        let ttl = TtlInspector::persistent(&env, &id, &KEY);
        let min = env.ledger().get().min_persistent_entry_ttl;

        assert_eq!(ttl.tier, StorageTier::Persistent);
        assert_eq!(ttl.live_until_ledger, env.ledger().sequence() + min - 1);
        assert_eq!(ttl.ttl(), min - 1);
        assert_eq!(ttl.min_ttl(), min);
        assert_eq!(ttl.status(), TtlStatus::Live);
    }

    #[test]
    fn a_new_temporary_entry_carries_the_minimum_temporary_ttl() {
        let (env, id, client) = setup();
        client.put_temporary(&1);

        let ttl = TtlInspector::temporary(&env, &id, &KEY);
        let min = env.ledger().get().min_temp_entry_ttl;

        assert_eq!(ttl.tier, StorageTier::Temporary);
        assert_eq!(ttl.live_until_ledger, env.ledger().sequence() + min - 1);
        assert_eq!(ttl.min_ttl(), min);
    }

    #[test]
    fn an_instance_key_reports_the_instance_entrys_ttl() {
        let (env, id, client) = setup();
        client.put_instance(&1);

        let key_ttl = TtlInspector::for_key(&env, &id, &KEY);
        let instance_ttl = TtlInspector::instance(&env, &id);

        assert_eq!(key_ttl.tier, StorageTier::Instance);
        assert_eq!(key_ttl.live_until_ledger, instance_ttl.live_until_ledger);
    }

    #[test]
    fn for_key_prefers_persistent_and_tier_constructors_disambiguate() {
        let (env, id, client) = setup();
        client.put_persistent(&1);
        client.put_temporary(&2);

        assert_eq!(
            TtlInspector::for_key(&env, &id, &KEY).tier,
            StorageTier::Persistent
        );
        assert_eq!(
            TtlInspector::temporary(&env, &id, &KEY).tier,
            StorageTier::Temporary
        );
    }

    #[test]
    fn try_for_key_returns_none_for_a_key_nothing_holds() {
        let (env, id, _client) = setup();

        assert!(TtlInspector::try_for_key(&env, &id, &symbol_short!("missing")).is_none());
    }

    #[test]
    #[should_panic(expected = "holds no entry for key")]
    fn for_key_panics_naming_the_missing_key() {
        let (env, id, _client) = setup();

        TtlInspector::for_key(&env, &id, &symbol_short!("missing"));
    }

    #[test]
    fn a_temporary_entry_past_its_ttl_is_expired_and_reads_empty() {
        let (env, id, client) = setup();
        client.put_temporary(&7);
        let ttl = TtlInspector::temporary(&env, &id, &KEY);

        env.ledger().set_sequence_number(ttl.expiration_ledger());

        let after = ttl.refresh(&env).expect("inspecting does not evict");
        assert_eq!(after.status(), TtlStatus::Expired);
        after.assert_expired();
        assert_eq!(client.get_temporary(), None);
    }

    #[test]
    fn a_temporary_entry_on_its_last_live_ledger_still_reads() {
        let (env, id, client) = setup();
        client.put_temporary(&7);
        let ttl = TtlInspector::temporary(&env, &id, &KEY);

        env.ledger().set_sequence_number(ttl.live_until_ledger);

        let at_boundary = ttl.refresh(&env).unwrap();
        at_boundary.assert_live();
        assert_eq!(at_boundary.ttl(), 0);
        assert_eq!(client.get_temporary(), Some(7));
    }

    #[test]
    fn a_persistent_entry_past_its_ttl_reports_archived() {
        let (env, id, client) = setup();
        client.put_persistent(&7);
        let ttl = TtlInspector::persistent(&env, &id, &KEY);

        env.ledger().set_sequence_number(ttl.expiration_ledger());

        let after = ttl.refresh(&env).unwrap();
        assert_eq!(after.status(), TtlStatus::Archived);
        assert_eq!(after.ttl(), 0);
    }

    #[test]
    fn an_instance_past_its_ttl_reports_archived() {
        let (env, id, client) = setup();
        client.put_instance(&1);
        let ttl = TtlInspector::instance(&env, &id);

        env.ledger().set_sequence_number(ttl.expiration_ledger());

        assert_eq!(ttl.refresh(&env).unwrap().status(), TtlStatus::Archived);
    }

    #[test]
    fn extend_ttl_on_a_persistent_entry_matches_the_prediction() {
        let (env, id, client) = setup();
        client.put_persistent(&1);
        let before = TtlInspector::persistent(&env, &id, &KEY);
        let threshold = before.ttl();
        let extend_to = before.ttl() + 10_000;

        assert_eq!(
            before.predict_extend_ttl(threshold, extend_to),
            Some(before.current_ledger + extend_to)
        );
        let extension = TtlExtension::observe(&env, &id, &KEY, || {
            client.bump_persistent(&threshold, &extend_to)
        });

        assert!(extension.was_extended());
        extension.assert_extended_to(extend_to);
        extension.assert_matches_extend_ttl(threshold, extend_to);
        assert_eq!(extension.extended_by(), 10_000);
    }

    #[test]
    fn extend_ttl_above_the_threshold_is_a_no_op() {
        let (env, id, client) = setup();
        client.put_persistent(&1);
        let before = TtlInspector::persistent(&env, &id, &KEY);
        let threshold = before.ttl() - 1;

        assert!(!before.needs_extension(threshold));
        let extension = TtlExtension::observe(&env, &id, &KEY, || {
            client.bump_persistent(&threshold, &(before.ttl() + 5_000))
        });

        extension.assert_not_extended();
        extension.assert_matches_extend_ttl(threshold, before.ttl() + 5_000);
    }

    #[test]
    fn extend_ttl_on_a_temporary_entry_after_advancing() {
        let (env, id, client) = setup();
        client.put_temporary(&1);
        let ttl = TtlInspector::temporary(&env, &id, &KEY);
        env.ledger()
            .set_sequence_number(ttl.current_ledger + ttl.ttl() / 2);

        let extension =
            TtlExtension::observe(&env, &id, &KEY, || client.bump_temporary(&100, &500));

        assert_eq!(extension.before.tier, StorageTier::Temporary);
        extension.assert_extended_to(500);
        extension.assert_matches_extend_ttl(100, 500);
    }

    #[test]
    fn extend_ttl_on_the_instance_moves_every_instance_key() {
        let (env, id, client) = setup();
        client.put_instance(&1);
        let before = TtlInspector::for_key(&env, &id, &KEY);
        let extend_to = before.ttl() + 1_000;

        let extension = TtlExtension::observe_from(&env, before, || {
            client.bump_instance(&extend_to, &extend_to)
        });

        extension.assert_extended_to(extend_to);
        assert_eq!(
            TtlInspector::instance(&env, &id).live_until_ledger,
            extension.after.live_until_ledger
        );
    }

    #[test]
    fn extend_to_is_clamped_to_the_network_ceiling() {
        let (env, id, client) = setup();
        client.put_persistent(&1);
        let before = TtlInspector::persistent(&env, &id, &KEY);
        let beyond = before.max_extend_to() + 1_000;

        assert_eq!(
            before.predict_extend_ttl(beyond, beyond),
            Some(before.max_live_until_ledger())
        );
        let extension =
            TtlExtension::observe(&env, &id, &KEY, || client.bump_persistent(&beyond, &beyond));

        extension.assert_extended_to(beyond);
        assert_eq!(
            extension.after.live_until_ledger,
            before.max_live_until_ledger()
        );
    }

    #[test]
    fn reading_an_expired_persistent_entry_restores_it_in_the_test_host() {
        let (env, id, client) = setup();
        client.put_persistent(&9);
        let ttl = TtlInspector::persistent(&env, &id, &KEY);
        env.ledger().set_sequence_number(ttl.expiration_ledger());

        assert_eq!(client.get_persistent(), Some(9));
        assert_eq!(
            ttl.refresh(&env).unwrap().status(),
            TtlStatus::Live,
            "the test host auto-restores an archived entry on access"
        );
    }

    #[test]
    fn display_names_tier_ledgers_and_status() {
        let (env, id, client) = setup();
        client.put_temporary(&1);

        let rendered = TtlInspector::temporary(&env, &id, &KEY).to_string();

        assert!(rendered.starts_with("temporary entry"), "{rendered}");
        assert!(
            rendered.contains("(live)") || rendered.ends_with("live)"),
            "{rendered}"
        );
    }
}
