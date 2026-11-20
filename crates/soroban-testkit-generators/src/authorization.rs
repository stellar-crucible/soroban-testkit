//! Strategies for Soroban authorization inputs.
use proptest::prelude::*;
use soroban_sdk::xdr::{ScAddress, ScVal, SorobanAddressCredentials, SorobanCredentials};

use crate::strategies::{address, AddressBytes};

/// Generates a valid address/nonce pair from a Soroban address.
pub fn address_with_nonce(
    address: AddressBytes,
) -> impl Strategy<Value = SorobanAddressCredentials> {
    (0i64..=i64::MAX).prop_map(move |nonce| SorobanAddressCredentials {
        address: ScAddress::from(address),
        nonce,
        signature_expiration_ledger: 0,
        signature: ScVal::Void,
    })
}

/// Generates strictly increasing non-negative authorization nonces.
pub fn sequential_nonces(count: usize) -> impl Strategy<Value = Vec<i64>> {
    prop::collection::vec(0i64..=i64::MAX / count.max(1) as i64, count).prop_map(|deltas| {
        let mut nonce = 0i64;
        deltas
            .into_iter()
            .map(|delta| {
                nonce = nonce.saturating_add(delta).saturating_add(1);
                nonce
            })
            .collect()
    })
}

/// Generates standard and address-based authorization credentials.
pub fn authorization_credential() -> impl Strategy<Value = SorobanCredentials> {
    prop_oneof![
        Just(SorobanCredentials::SourceAccount),
        address().prop_map(|address| {
            SorobanCredentials::Address(SorobanAddressCredentials {
                address: ScAddress::from(address),
                nonce: 0,
                signature_expiration_ledger: 0,
                signature: ScVal::Void,
            })
        }),
        address().prop_map(|address| {
            SorobanCredentials::AddressV2(SorobanAddressCredentials {
                address: ScAddress::from(address),
                nonce: 0,
                signature_expiration_ledger: 0,
                signature: ScVal::Void,
            })
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::test_runner::TestRunner;
    use soroban_sdk::xdr::{ReadXdr as _, WriteXdr as _};

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn generated_nonce_sequences_are_strictly_increasing(nonces in sequential_nonces(8)) {
            prop_assert_eq!(nonces.len(), 8);
            prop_assert!(nonces.iter().all(|nonce| *nonce > 0));
            prop_assert!(nonces.windows(2).all(|pair| pair[0] < pair[1]));
        }

        #[test]
        fn credentials_encode_as_xdr(credential in authorization_credential()) {
            let bytes = credential.to_xdr(soroban_sdk::xdr::Limits::none()).unwrap();
            let decoded = SorobanCredentials::from_xdr(bytes.as_slice(), soroban_sdk::xdr::Limits::none()).unwrap();
            prop_assert_eq!(decoded, credential);
        }
    }

    #[test]
    fn nonce_strategy_includes_small_values() {
        let mut runner = TestRunner::default();
        runner
            .run(&sequential_nonces(2), |nonces| {
                prop_assert!(nonces[0] >= 1);
                prop_assert!(nonces[1] > nonces[0]);
                Ok(())
            })
            .unwrap();
    }
}
