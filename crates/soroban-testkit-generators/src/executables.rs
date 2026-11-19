//! Property testing strategies for Soroban contract executables, WASM hashes, and deployer salts.
//!
//! Factory contracts and deployment utilities accept WASM bytecode hashes or raw
//! contract executables to deploy child contracts dynamically. These strategies
//! generate valid bytecode hashes, `ContractExecutable` enum descriptors, and
//! random 32-byte deployer salts to test deterministic address calculation and
//! factory deployment routines.

use proptest::prelude::*;
use soroban_sdk::xdr::{
    ContractExecutable, ContractExecutableExternalRef, Hash, ScAddress, ScString, StringM,
};

use crate::strategies::address;
use crate::tuples::{byte_array_32, ByteArrayStrategy};

/// Generates a valid 32-byte WASM bytecode hash.
///
/// Each byte shrinks towards zero independently.
pub fn contract_executable_wasm_hash() -> ByteArrayStrategy<32> {
    byte_array_32()
}

/// Generates a random 32-byte salt for deterministic contract deployment.
///
/// The salt can be turned into `soroban_sdk::BytesN<32>` via `BytesN::from_array(&env, &salt)`
/// and supplied to `env.deployer().with_address(deployer, salt)` or
/// `env.deployer().with_current_contract(salt)`.
pub fn deployer_salt() -> ByteArrayStrategy<32> {
    byte_array_32()
}

/// Generates `soroban_sdk::xdr::ContractExecutable` enum variants.
///
/// Produces `ContractExecutable::Wasm` variants carrying valid 32-byte hashes,
/// `ContractExecutable::StellarAsset` variants, and `ContractExecutable::ExternalRef` variants.
pub fn contract_executable() -> impl Strategy<Value = ContractExecutable> {
    prop_oneof![
        4 => contract_executable_wasm_hash().prop_map(|h| ContractExecutable::Wasm(Hash(h))),
        2 => Just(ContractExecutable::StellarAsset),
        1 => (address(), "[a-zA-Z0-9_]{1,16}").prop_filter_map("valid external ref tag", |(addr, tag_str)| {
            let m = StringM::try_from(tag_str).ok()?;
            Some(ContractExecutable::ExternalRef(ContractExecutableExternalRef {
                executable_owner: ScAddress::from(addr),
                tag: ScString(m),
            }))
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Address, BytesN, Env};

    proptest! {
        #[test]
        fn test_wasm_hash_generation(hash in contract_executable_wasm_hash()) {
            assert_eq!(hash.len(), 32);
            let env = Env::default();
            let bytes_n = BytesN::from_array(&env, &hash);
            assert_eq!(bytes_n.to_array(), hash);
        }

        #[test]
        fn test_contract_executable_variants(exec in contract_executable()) {
            match exec {
                ContractExecutable::Wasm(Hash(h)) => {
                    assert_eq!(h.len(), 32);
                }
                ContractExecutable::StellarAsset => {
                    // Valid variant
                }
                ContractExecutable::ExternalRef(ext) => {
                    assert!(!ext.tag.0.is_empty());
                }
            }
        }

        #[test]
        fn test_deterministic_address_calculation_with_salt(
            salt_a in deployer_salt(),
            salt_b in deployer_salt(),
        ) {
            let env = Env::default();
            let deployer = Address::generate(&env);

            let salt_a_n = BytesN::from_array(&env, &salt_a);
            let salt_b_n = BytesN::from_array(&env, &salt_b);

            // Deterministic calculation: same deployer + same salt -> identical predicted address
            let predicted_1 = env
                .deployer()
                .with_address(deployer.clone(), salt_a_n.clone())
                .deployed_address();
            let predicted_2 = env
                .deployer()
                .with_address(deployer.clone(), salt_a_n)
                .deployed_address();

            assert_eq!(predicted_1, predicted_2);

            // Different salts -> different predicted addresses
            if salt_a != salt_b {
                let predicted_b = env
                    .deployer()
                    .with_address(deployer, salt_b_n)
                    .deployed_address();
                assert_ne!(predicted_1, predicted_b);
            }
        }
    }
}
