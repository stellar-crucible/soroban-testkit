use core::sync::atomic::{AtomicU64, Ordering};

use proptest::prelude::*;
use soroban_sdk::xdr::{AccountId, ContractId, Hash, PublicKey, ScAddress, Uint256};
use soroban_sdk::{Address, Env, TryFromVal as _};

/// The two address formats Soroban accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddressType {
    /// An ed25519 account, the `G...` form.
    Account,
    /// A contract id, the `C...` form.
    Contract,
}

/// A generated Soroban address: 32 raw bytes tagged with the format they
/// belong to.
///
/// Both formats are opaque 32-byte values on the wire, so the tag is what
/// separates an ed25519 public key from a contract id. Turn it into a host
/// [`Address`] with [`AddressBytes::to_address`], or into its XDR form with
/// `ScAddress::from`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AddressBytes {
    address_type: AddressType,
    bytes: [u8; 32],
}

impl AddressBytes {
    /// The format this address was generated as.
    pub fn address_type(&self) -> AddressType {
        self.address_type
    }

    /// The raw 32 bytes, as an ed25519 public key or a contract id.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// The raw 32 bytes, consuming the address.
    pub fn into_bytes(self) -> [u8; 32] {
        self.bytes
    }

    /// Rebuild the address as a host value owned by `env`.
    pub fn to_address(self, env: &Env) -> Address {
        Address::try_from_val(env, &ScAddress::from(self))
            .expect("a 32-byte address is a valid ScAddress")
    }
}

impl From<AddressBytes> for ScAddress {
    fn from(address: AddressBytes) -> Self {
        match address.address_type {
            AddressType::Account => ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                Uint256(address.bytes),
            ))),
            AddressType::Contract => ScAddress::Contract(ContractId(Hash(address.bytes))),
        }
    }
}

/// Hands every generated address a distinct suffix, so two cases in one run
/// cannot collide even if their random prefixes do.
static NEXT_ADDRESS_NONCE: AtomicU64 = AtomicU64::new(1);

/// Generates either an account or a contract address.
pub fn address() -> impl Strategy<Value = AddressBytes> {
    prop_oneof![account_address(), contract_address()]
}

/// Generates an ed25519 account address.
pub fn account_address() -> impl Strategy<Value = AddressBytes> {
    address_of_type(AddressType::Account)
}

/// Generates a contract address.
pub fn contract_address() -> impl Strategy<Value = AddressBytes> {
    address_of_type(AddressType::Contract)
}

/// Generates an address of the requested format.
pub fn address_of_type(address_type: AddressType) -> impl Strategy<Value = AddressBytes> {
    any::<[u8; 32]>().prop_map(move |mut bytes| {
        let nonce = NEXT_ADDRESS_NONCE.fetch_add(1, Ordering::Relaxed);
        bytes[24..].copy_from_slice(&nonce.to_be_bytes());
        AddressBytes {
            address_type,
            bytes,
        }
    })
}

pub fn token_amount() -> impl Strategy<Value = i128> {
    0i128..=1_000_000_000_000_000i128
}

pub fn ledger_sequence() -> impl Strategy<Value = u32> {
    1u32..=10_000_000u32
}

pub fn timestamp() -> impl Strategy<Value = u64> {
    1_600_000_000u64..=2_000_000_000u64
}

#[cfg(test)]
mod tests {
    use super::{
        account_address, address, address_of_type, contract_address, ledger_sequence, timestamp,
        token_amount, AddressType,
    };
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::xdr::ScAddress;
    use soroban_sdk::Env;
    use std::collections::HashSet;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn token_amounts_stay_non_negative(amount in token_amount()) {
            prop_assert!(amount >= 0);
        }

        #[test]
        fn token_amounts_stay_below_the_supply_cap(amount in token_amount()) {
            prop_assert!(amount <= 1_000_000_000_000_000);
        }

        #[test]
        fn ledger_sequences_are_never_zero(seq in ledger_sequence()) {
            prop_assert!(seq >= 1);
            prop_assert!(seq <= 10_000_000);
        }

        #[test]
        fn timestamps_fall_inside_the_stellar_era(ts in timestamp()) {
            prop_assert!(ts >= 1_600_000_000);
            prop_assert!(ts <= 2_000_000_000);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        #[test]
        fn account_addresses_are_32_byte_ed25519_keys(address in account_address()) {
            prop_assert_eq!(address.address_type(), AddressType::Account);
            prop_assert_eq!(address.as_bytes().len(), 32);
            prop_assert!(matches!(ScAddress::from(address), ScAddress::Account(_)));
        }

        #[test]
        fn contract_addresses_are_32_byte_contract_ids(address in contract_address()) {
            prop_assert_eq!(address.address_type(), AddressType::Contract);
            prop_assert_eq!(address.as_bytes().len(), 32);
            prop_assert!(matches!(ScAddress::from(address), ScAddress::Contract(_)));
        }

        #[test]
        fn the_type_filter_only_yields_the_requested_format(address in address_of_type(AddressType::Contract)) {
            prop_assert_eq!(address.address_type(), AddressType::Contract);
        }

        #[test]
        fn the_default_strategy_yields_one_of_the_two_formats(address in address()) {
            prop_assert!(matches!(
                address.address_type(),
                AddressType::Account | AddressType::Contract
            ));
        }
    }

    #[test]
    fn generated_addresses_convert_to_host_addresses() {
        let env = Env::default();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(32));
        runner
            .run(
                &(account_address(), contract_address()),
                |(account, contract)| {
                    let account = ScAddress::from(account.to_address(&env));
                    let contract = ScAddress::from(contract.to_address(&env));
                    prop_assert!(matches!(account, ScAddress::Account(_)));
                    prop_assert!(matches!(contract, ScAddress::Contract(_)));
                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn a_single_run_never_generates_a_duplicate_address() {
        let mixed = prop_oneof![account_address(), contract_address()];
        let mut runner = TestRunner::new(ProptestConfig::with_cases(32));
        runner
            .run(&proptest::collection::vec(mixed, 64), |addresses| {
                let unique: HashSet<_> =
                    addresses.iter().map(|address| address.as_bytes()).collect();
                prop_assert_eq!(unique.len(), addresses.len());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn strategies_compose_into_tuples() {
        let mut runner = TestRunner::default();
        runner
            .run(&(token_amount(), ledger_sequence()), |(amount, seq)| {
                prop_assert!(amount >= 0);
                prop_assert!(seq >= 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn filtered_strategy_only_yields_sufficient_balances() {
        let strategy = (token_amount(), token_amount())
            .prop_filter("sender must have sufficient balance", |(a, b)| a >= b);

        let mut runner = TestRunner::default();
        runner
            .run(&strategy, |(sender_balance, amount)| {
                prop_assert!(sender_balance >= amount);
                Ok(())
            })
            .unwrap();
    }
}
