//! Value types and range bounds shared by every generator backend.
//!
//! Nothing in this module depends on `proptest` or `arbitrary`, so the
//! [`strategies`](crate::strategies) module (behind the `proptest` feature) and
//! the [`arbitrary`](crate::arbitrary) module (behind the `arbitrary` feature)
//! agree on what a valid token amount, ledger sequence, timestamp or address
//! is, whichever of the two features a crate turns on.

use soroban_sdk::xdr::{AccountId, ContractId, Hash, PublicKey, ScAddress, Uint256};
use soroban_sdk::{Address, Env, TryFromVal as _};

/// Largest token amount a generator produces: 10^15 stroops-style units.
pub const TOKEN_AMOUNT_MAX: i128 = 1_000_000_000_000_000;

/// Smallest ledger sequence a generator produces. Sequence zero never closes.
pub const LEDGER_SEQUENCE_MIN: u32 = 1;

/// Largest ledger sequence a generator produces.
pub const LEDGER_SEQUENCE_MAX: u32 = 10_000_000;

/// Earliest unix timestamp a generator produces (September 2020).
pub const TIMESTAMP_MIN: u64 = 1_600_000_000;

/// Latest unix timestamp a generator produces (May 2033).
pub const TIMESTAMP_MAX: u64 = 2_000_000_000;

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
    /// Crate-internal constructor used by the generator backends.
    #[cfg_attr(
        not(any(feature = "proptest", feature = "arbitrary")),
        allow(dead_code)
    )]
    pub(crate) fn new(address_type: AddressType, bytes: [u8; 32]) -> Self {
        Self {
            address_type,
            bytes,
        }
    }

    /// Build an address from its format tag and bytes.
    ///
    /// Crate-internal: the public API generates addresses, and a hand-built one
    /// is only useful to a test that needs a predictable value.
    #[cfg(test)]
    pub(crate) fn from_parts(address_type: AddressType, bytes: [u8; 32]) -> Self {
        Self::new(address_type, bytes)
    }

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
