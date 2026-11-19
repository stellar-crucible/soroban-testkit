//! Assertions for storage that should no longer exist.
//!
//! Closing an account, burning a balance or cancelling an escrow usually ends
//! with the contract deleting its entries. These assertions prove that the
//! entries are *gone*, not merely rewritten: a key holding `0`, an empty
//! `Option::None`, an empty map or any other tombstone value is still a live
//! ledger entry and fails them.
//!
//! Both assertions inspect all three storage tiers — instance, persistent and
//! temporary — unless one tier is named explicitly, and a failure lists every
//! surviving entry with its tier, rendered value and XDR bytes.
//!
//! ```
//! use soroban_testkit_assert::{assert_storage_empty, assert_storage_key_deleted};
//! use soroban_testkit_assert::storage::StorageTier;
//! # use soroban_sdk::{Address, Env};
//!
//! # fn example(env: &Env, contract: &Address) {
//! // The key is gone from instance, persistent and temporary storage.
//! assert_storage_key_deleted!(env, contract, "count");
//!
//! // The key is gone from one tier; another tier may still hold it.
//! assert_storage_key_deleted!(env, contract, "count", StorageTier::Persistent);
//!
//! // The contract's whole storage, or one tier of it, is empty.
//! assert_storage_empty!(env, contract);
//! assert_storage_empty!(env, contract, StorageTier::Temporary);
//! # }
//! ```

use soroban_sdk::{Address, Env};
use soroban_testkit_core::storage::{StorageEntry, StorageSnapshot};

pub use soroban_testkit_core::storage::StorageTier;

/// Assert that `key` exists in none of the contract's storage tiers.
///
/// The key is the rendered form a [`StorageSnapshot`] reports — a symbol's
/// text, a number, `[a, b]` for a vector, `{k: v}` for a map — the same form
/// [`assert_storage_tier`](soroban_testkit_core::assert_storage_tier) takes.
///
/// # Panics
///
/// Panics if any tier still holds the key, showing every surviving entry.
pub fn assert_storage_key_deleted(env: &Env, contract: &Address, key: &str) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors: Vec<&StorageEntry> = snapshot
        .entries()
        .iter()
        .filter(|entry| entry.key == key)
        .collect();

    assert!(
        survivors.is_empty(),
        "Expected storage key {:?} to be deleted from all tiers, but it still exists:\n{}",
        key,
        describe(&survivors),
    );
}

/// Assert that `key` no longer exists in one storage tier.
///
/// The key may still live in another tier; only `tier` is inspected.
///
/// # Panics
///
/// Panics if `tier` still holds the key, showing the surviving entry.
pub fn assert_storage_key_deleted_in(env: &Env, contract: &Address, key: &str, tier: StorageTier) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors: Vec<&StorageEntry> = snapshot
        .in_tier(tier)
        .into_iter()
        .filter(|entry| entry.key == key)
        .collect();

    assert!(
        survivors.is_empty(),
        "Expected storage key {:?} to be deleted from the {} tier, but it still exists:\n{}",
        key,
        tier.as_str(),
        describe(&survivors),
    );
}

/// Assert that the contract holds no live storage entry in any tier.
///
/// A contract that never wrote storage passes, as does one whose keys were
/// all deleted. Entries rewritten to zero, `None` or another empty value do
/// not pass: they are still stored.
///
/// # Panics
///
/// Panics if any tier holds an entry, listing every survivor with its bytes.
pub fn assert_storage_empty(env: &Env, contract: &Address) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors: Vec<&StorageEntry> = snapshot.entries().iter().collect();

    assert!(
        survivors.is_empty(),
        "Expected contract storage to be empty, but {} entr{} survived:\n{}",
        survivors.len(),
        if survivors.len() == 1 { "y" } else { "ies" },
        describe(&survivors),
    );
}

/// Assert that one storage tier of the contract holds no live entry.
///
/// The other tiers are not inspected and may hold entries.
///
/// # Panics
///
/// Panics if `tier` holds an entry, showing it with its value and bytes.
pub fn assert_storage_empty_in(env: &Env, contract: &Address, tier: StorageTier) {
    let snapshot = StorageSnapshot::capture(env, contract);
    let survivors = snapshot.in_tier(tier);

    assert!(
        survivors.is_empty(),
        "Expected the {} storage tier to be empty, but {} entr{} survived:\n{}",
        tier.as_str(),
        survivors.len(),
        if survivors.len() == 1 { "y" } else { "ies" },
        describe(&survivors),
    );
}

/// Asserts that a storage key no longer exists in any tier.
///
/// Pass a [`StorageTier`] as a fourth argument to inspect one tier only.
///
/// # Example
///
/// ```rust
/// use soroban_testkit_assert::{assert_storage_key_deleted, storage::StorageTier};
/// # use soroban_sdk::{Address, Env};
/// # fn example(env: &Env, contract: &Address) {
/// assert_storage_key_deleted!(env, contract, "balance");
/// assert_storage_key_deleted!(env, contract, "balance", StorageTier::Persistent);
/// # }
/// ```
#[macro_export]
macro_rules! assert_storage_key_deleted {
    ($env:expr, $contract:expr, $key:expr $(,)?) => {
        $crate::storage::assert_storage_key_deleted($env, $contract, $key)
    };
    ($env:expr, $contract:expr, $key:expr, $tier:expr $(,)?) => {
        $crate::storage::assert_storage_key_deleted_in($env, $contract, $key, $tier)
    };
}

/// Asserts that a contract's storage, or one tier of it, is empty.
///
/// Pass a [`StorageTier`] as a third argument to inspect one tier only.
///
/// # Example
///
/// ```rust
/// use soroban_testkit_assert::{assert_storage_empty, storage::StorageTier};
/// # use soroban_sdk::{Address, Env};
/// # fn example(env: &Env, contract: &Address) {
/// assert_storage_empty!(env, contract);
/// assert_storage_empty!(env, contract, StorageTier::Temporary);
/// # }
/// ```
#[macro_export]
macro_rules! assert_storage_empty {
    ($env:expr, $contract:expr $(,)?) => {
        $crate::storage::assert_storage_empty($env, $contract)
    };
    ($env:expr, $contract:expr, $tier:expr $(,)?) => {
        $crate::storage::assert_storage_empty_in($env, $contract, $tier)
    };
}

/// How much of a surviving value's XDR to quote in a failure message.
const MAX_QUOTED_BYTES: usize = 64;

fn describe(entries: &[&StorageEntry]) -> String {
    entries
        .iter()
        .map(|entry| format!("  {}", describe_entry(entry)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn describe_entry(entry: &StorageEntry) -> String {
    let bytes = entry.value_bytes();
    format!(
        "{} = {} [{} byte{}: {}]",
        entry.qualified(),
        entry.value,
        bytes.len(),
        if bytes.len() == 1 { "" } else { "s" },
        hex(bytes),
    )
}

fn hex(bytes: &[u8]) -> String {
    let mut rendered = String::with_capacity(MAX_QUOTED_BYTES * 2 + 1);
    for byte in bytes.iter().take(MAX_QUOTED_BYTES) {
        rendered.push_str(&format!("{byte:02x}"));
    }
    if bytes.len() > MAX_QUOTED_BYTES {
        rendered.push('…');
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::StorageTier;
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env, Symbol,
    };

    #[contract]
    pub struct Store;

    #[contractimpl]
    impl Store {
        pub fn seed(env: Env) {
            env.storage().persistent().set(&count_key(&env), &42u32);
            env.storage().temporary().set(&ttl_key(&env), &7u32);
            env.storage().instance().set(&admin_key(), &1u32);
        }

        pub fn erase_count(env: Env) {
            env.storage().persistent().remove(&count_key(&env));
        }

        pub fn store_zero(env: Env) {
            env.storage().persistent().set(&count_key(&env), &0u32);
        }

        pub fn store_none(env: Env) {
            env.storage()
                .persistent()
                .set(&count_key(&env), &Option::<u32>::None);
        }

        pub fn store_empty_map(env: Env) {
            env.storage().persistent().set(
                &count_key(&env),
                &soroban_sdk::Map::<Symbol, u32>::new(&env),
            );
        }

        pub fn clear(env: Env) {
            env.storage().persistent().remove(&count_key(&env));
            env.storage().temporary().remove(&ttl_key(&env));
            env.storage().instance().remove(&admin_key());
        }
    }

    fn count_key(env: &Env) -> Symbol {
        Symbol::new(env, "count")
    }

    fn ttl_key(env: &Env) -> Symbol {
        Symbol::new(env, "ttl")
    }

    fn admin_key() -> Symbol {
        symbol_short!("admin")
    }

    fn registered() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(Store, ());
        (env, contract)
    }

    fn seeded() -> (Env, Address) {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).seed();
        (env, contract)
    }

    #[test]
    fn deleted_key_is_absent_from_every_tier() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).erase_count();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    fn deleted_key_in_a_tier_passes_while_other_tiers_hold_keys() {
        let (env, contract) = seeded();
        let client = StoreClient::new(&env, &contract);
        client.erase_count();

        assert_storage_key_deleted!(&env, &contract, "count", StorageTier::Persistent);

        // "ttl" never existed in the persistent tier, so naming that tier passes
        // even though the key is alive in temporary storage.
        assert_storage_key_deleted!(&env, &contract, "ttl", StorageTier::Persistent);
    }

    #[test]
    #[should_panic(expected = "persistent:count = 42")]
    fn existing_key_fails_across_all_tiers() {
        let (env, contract) = seeded();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    #[should_panic(expected = "to be deleted from the persistent tier")]
    fn existing_key_fails_in_the_named_tier() {
        let (env, contract) = seeded();

        assert_storage_key_deleted!(&env, &contract, "count", StorageTier::Persistent);
    }

    #[test]
    #[should_panic(expected = "persistent:count = 0")]
    fn a_key_rewritten_to_zero_is_not_deleted() {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).store_zero();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    #[should_panic(expected = "persistent:count = void")]
    fn a_key_holding_option_none_is_not_deleted() {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).store_none();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    #[should_panic(expected = "persistent:count = {}")]
    fn a_key_holding_an_empty_value_is_not_deleted() {
        let (env, contract) = registered();
        StoreClient::new(&env, &contract).store_empty_map();

        assert_storage_key_deleted!(&env, &contract, "count");
    }

    #[test]
    fn an_untouched_contract_has_empty_storage() {
        let (env, contract) = registered();

        assert_storage_empty!(&env, &contract);
        assert_storage_empty!(&env, &contract, StorageTier::Instance);
        assert_storage_empty!(&env, &contract, StorageTier::Persistent);
        assert_storage_empty!(&env, &contract, StorageTier::Temporary);
    }

    #[test]
    fn a_cleared_contract_has_empty_storage() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).clear();

        assert_storage_empty!(&env, &contract);
    }

    #[test]
    fn an_empty_tier_passes_while_other_tiers_hold_keys() {
        let (env, contract) = seeded();
        StoreClient::new(&env, &contract).erase_count();

        assert_storage_empty!(&env, &contract, StorageTier::Persistent);

        // The other tiers are untouched and still hold their keys.
        assert_storage_key_deleted!(&env, &contract, "ttl", StorageTier::Persistent);
        assert_storage_key_deleted!(&env, &contract, "admin", StorageTier::Persistent);
    }

    #[test]
    #[should_panic(expected = "3 entries survived")]
    fn a_non_empty_contract_lists_every_survivor() {
        let (env, contract) = seeded();

        assert_storage_empty!(&env, &contract);
    }

    #[test]
    #[should_panic(expected = "temporary:ttl = 7 [8 bytes:")]
    fn survivors_are_reported_with_their_bytes() {
        let (env, contract) = seeded();

        assert_storage_empty!(&env, &contract);
    }

    #[test]
    #[should_panic(expected = "temporary storage tier to be empty")]
    fn a_non_empty_tier_fails() {
        let (env, contract) = seeded();

        assert_storage_empty!(&env, &contract, StorageTier::Temporary);
    }

    #[test]
    fn a_non_contract_address_captures_as_empty() {
        let env = Env::default();
        let account = Address::generate(&env);

        assert_storage_empty!(&env, &account);
        assert_storage_key_deleted!(&env, &account, "count");
    }
}
