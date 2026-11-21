//! Proptest strategies for Stellar strkey-encoded addresses.
//!
//! Contracts that accept addresses as strings (a `G...` ed25519 account id or a
//! `C...` contract id) need two kinds of test input: a well-formed strkey that
//! decodes, and a corrupted one that a parser must reject *cleanly* rather than
//! panic on or silently accept.
//!
//! The strategies here produce both. Encoding is implemented locally so the
//! strategies do not need a separate strkey dependency:
//!
//! * version byte (`6 << 3` for ed25519 accounts, `2 << 3` for contracts),
//! * 32 payload bytes,
//! * CRC16-XModem over the version and payload, stored little-endian,
//! * RFC 4648 base32 without padding (35 bytes encode to exactly 56 chars).
//!
//! Payload bytes shrink towards zero, so a failing property reports the
//! canonical zero-payload strkey (`[`zero_account_strkey`]` /
//! [`zero_contract_strkey`]) rather than a random 56-character value.
//!
//! ```rust
//! use proptest::prelude::*;
//! use soroban_testkit_generators::strkey::{account_strkey, corrupted_strkey, Strkey};
//!
//! proptest! {
//!     fn a_parser_accepts_valid_and_rejects_corrupted(
//!         valid in account_strkey(),
//!         corrupted in corrupted_strkey(),
//!     ) {
//!         prop_assert!(Strkey::decode(&valid).is_ok());
//!         prop_assert!(Strkey::decode(&corrupted).is_err());
//!     }
//! }
//! ```

use std::fmt;

use proptest::prelude::*;
use soroban_sdk::{
    xdr::{AccountId, ContractId, Hash, PublicKey, ScAddress, Uint256},
    Address, Env, TryFromVal as _,
};

/// The two strkey address formats Soroban accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StrkeyKind {
    /// Ed25519 account, encoded as `G...`.
    Account,
    /// Contract id, encoded as `C...`.
    Contract,
}

impl StrkeyKind {
    /// The version byte prefixed to the payload before checksumming.
    pub fn version(self) -> u8 {
        match self {
            StrkeyKind::Account => 6 << 3,
            StrkeyKind::Contract => 2 << 3,
        }
    }

    fn from_version(version: u8) -> Option<Self> {
        match version {
            48 => Some(StrkeyKind::Account),
            16 => Some(StrkeyKind::Contract),
            _ => None,
        }
    }
}

/// Why a string is not a valid strkey.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrkeyError {
    /// A strkey is exactly 56 characters; anything else is rejected first.
    InvalidLength { length: usize },
    /// A character outside the RFC 4648 base32 alphabet.
    InvalidCharacter { index: usize, character: char },
    /// The trailing CRC16 does not cover the version and payload.
    InvalidChecksum { expected: u16, actual: u16 },
    /// A well-formed strkey with a version this toolkit does not model.
    UnknownVersion(u8),
}

impl fmt::Display for StrkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StrkeyError::InvalidLength { length } => {
                write!(f, "strkey must be 56 characters, got {length}")
            }
            StrkeyError::InvalidCharacter { index, character } => {
                write!(f, "invalid base32 character {character:?} at index {index}")
            }
            StrkeyError::InvalidChecksum { expected, actual } => write!(
                f,
                "CRC16 checksum mismatch: expected {expected:#06x}, found {actual:#06x}"
            ),
            StrkeyError::UnknownVersion(version) => {
                write!(f, "unknown strkey version byte {version}")
            }
        }
    }
}

impl std::error::Error for StrkeyError {}

/// A decoded strkey: its format tag and 32 payload bytes.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Strkey {
    kind: StrkeyKind,
    bytes: [u8; 32],
}

impl fmt::Debug for Strkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Strkey")
            .field("kind", &self.kind)
            .field("encoded", &self.encoded())
            .finish()
    }
}

impl Strkey {
    /// Build a strkey from a format tag and its 32-byte payload.
    pub fn new(kind: StrkeyKind, bytes: [u8; 32]) -> Self {
        Self { kind, bytes }
    }

    /// The all-zero payload for `kind` — the canonical shrink target.
    pub fn zero(kind: StrkeyKind) -> Self {
        Self::new(kind, [0u8; 32])
    }

    pub fn kind(&self) -> StrkeyKind {
        self.kind
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    /// The 56-character strkey string.
    pub fn encoded(&self) -> String {
        encode_bytes(self.kind.version(), &self.bytes)
    }

    /// Decode and validate a strkey string.
    ///
    /// Validation is ordered so a corrupted input fails with the most local
    /// reason: length, then alphabet, then checksum, then version.
    pub fn decode(encoded: &str) -> Result<Self, StrkeyError> {
        if encoded.len() != 56 {
            return Err(StrkeyError::InvalidLength {
                length: encoded.len(),
            });
        }

        let mut bytes = Vec::with_capacity(35);
        let mut buffer: u32 = 0;
        let mut bits = 0u32;
        for (index, character) in encoded.chars().enumerate() {
            let Some(value) = base32_value(character) else {
                return Err(StrkeyError::InvalidCharacter { index, character });
            };
            buffer = (buffer << 5) | u32::from(value);
            bits += 5;
            if bits >= 8 {
                bits -= 8;
                bytes.push((buffer >> bits) as u8);
                buffer &= (1 << bits) - 1;
            }
        }

        if bytes.len() != 35 {
            return Err(StrkeyError::InvalidLength {
                length: encoded.len(),
            });
        }

        let actual = u16::from_le_bytes([bytes[33], bytes[34]]);
        let expected = crc16_xmodem(&bytes[..33]);
        if actual != expected {
            return Err(StrkeyError::InvalidChecksum { expected, actual });
        }

        let version = bytes[0];
        let kind = StrkeyKind::from_version(version).ok_or(StrkeyError::UnknownVersion(version))?;
        let mut payload = [0u8; 32];
        payload.copy_from_slice(&bytes[1..33]);
        Ok(Self::new(kind, payload))
    }

    /// Rebuild this strkey as a host [`Address`] owned by `env`.
    pub fn to_address(&self, env: &Env) -> Address {
        let sc_address = match self.kind {
            StrkeyKind::Account => ScAddress::Account(AccountId(PublicKey::PublicKeyTypeEd25519(
                Uint256(self.bytes),
            ))),
            StrkeyKind::Contract => ScAddress::Contract(ContractId(Hash(self.bytes))),
        };
        Address::try_from_val(env, &sc_address).expect("a decoded strkey is a valid address")
    }
}

/// The canonical zero-payload account strkey (`G` followed by zeros and its
/// CRC16), the value [`account_strkey`] shrinks towards.
pub fn zero_account_strkey() -> String {
    Strkey::zero(StrkeyKind::Account).encoded()
}

/// The canonical zero-payload contract strkey, the value [`contract_strkey`]
/// shrinks towards.
pub fn zero_contract_strkey() -> String {
    Strkey::zero(StrkeyKind::Contract).encoded()
}

fn payload_strategy() -> impl Strategy<Value = [u8; 32]> {
    proptest::array::uniform32(any::<u8>())
}

/// Generate valid 56-character ed25519 account strkeys (`G...`).
///
/// The 32 payload bytes shrink towards zero, so the smallest counterexample is
/// [`zero_account_strkey`].
pub fn account_strkey() -> impl Strategy<Value = String> {
    payload_strategy().prop_map(|bytes| Strkey::new(StrkeyKind::Account, bytes).encoded())
}

/// Generate valid 56-character contract strkeys (`C...`), shrinking towards
/// [`zero_contract_strkey`].
pub fn contract_strkey() -> impl Strategy<Value = String> {
    payload_strategy().prop_map(|bytes| Strkey::new(StrkeyKind::Contract, bytes).encoded())
}

/// Generate either a valid account or contract strkey.
pub fn valid_strkey() -> impl Strategy<Value = String> {
    prop_oneof![account_strkey(), contract_strkey()]
}

/// Generate strkeys that every strkey parser must reject.
///
/// Two corruption families are covered:
///
/// * an invalid base32 character (`0`, `1`, `8`, `9`, `!`, or a lowercase
///   letter) at a random position, which fails alphabet validation, and
/// * a valid payload whose stored CRC16 has a bit flipped, which fails
///   checksum validation.
pub fn corrupted_strkey() -> impl Strategy<Value = String> {
    prop_oneof![
        2 => (payload_strategy(), any::<usize>()).prop_map(|(bytes, index)| {
            corrupt_character(&Strkey::new(StrkeyKind::Account, bytes), index)
        }),
        1 => (payload_strategy(), any::<usize>()).prop_map(|(bytes, index)| {
            corrupt_character(&Strkey::new(StrkeyKind::Contract, bytes), index)
        }),
        2 => payload_strategy()
            .prop_map(|bytes| corrupt_checksum(&Strkey::new(StrkeyKind::Account, bytes))),
        1 => payload_strategy()
            .prop_map(|bytes| corrupt_checksum(&Strkey::new(StrkeyKind::Contract, bytes))),
    ]
}

fn corrupt_character(value: &Strkey, index: usize) -> String {
    const INVALID: [char; 6] = ['0', '1', '8', '9', '!', 'a'];
    let mut chars: Vec<char> = value.encoded().chars().collect();
    let position = index % chars.len();
    chars[position] = INVALID[position % INVALID.len()];
    chars.into_iter().collect()
}

fn corrupt_checksum(value: &Strkey) -> String {
    let version = value.kind.version();
    let mut body = Vec::with_capacity(33);
    body.push(version);
    body.extend_from_slice(&value.bytes);
    let checksum = crc16_xmodem(&body) ^ 0x0001;
    body.extend_from_slice(&checksum.to_le_bytes());
    base32_encode(&body)
}

fn encode_bytes(version: u8, payload: &[u8; 32]) -> String {
    let mut body = Vec::with_capacity(35);
    body.push(version);
    body.extend_from_slice(payload);
    let checksum = crc16_xmodem(&body);
    body.extend_from_slice(&checksum.to_le_bytes());
    base32_encode(&body)
}

fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in data {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_value(character: char) -> Option<u8> {
    if !character.is_ascii() {
        return None;
    }
    BASE32
        .iter()
        .position(|candidate| *candidate as char == character)
        .map(|index| index as u8)
}

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;
    for byte in data {
        buffer = (buffer << 8) | u32::from(*byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let index = ((buffer >> bits) & 0x1f) as usize;
            out.push(BASE32[index] as char);
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        let index = ((buffer << (5 - bits)) & 0x1f) as usize;
        out.push(BASE32[index] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        account_strkey, contract_strkey, corrupted_strkey, valid_strkey, zero_account_strkey,
        zero_contract_strkey, Strkey, StrkeyError, StrkeyKind, BASE32,
    };
    use proptest::prelude::*;
    use proptest::test_runner::{TestError, TestRunner};
    use soroban_sdk::Env;

    #[test]
    fn every_valid_account_strkey_is_56_base32_chars_with_a_checksum() {
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        runner
            .run(&account_strkey(), |encoded| {
                prop_assert_eq!(encoded.len(), 56);
                prop_assert!(encoded.starts_with('G'));
                prop_assert!(encoded.chars().all(|ch| BASE32.contains(&(ch as u8))));
                let decoded = Strkey::decode(&encoded).expect("generated strkeys decode");
                prop_assert_eq!(decoded.kind(), StrkeyKind::Account);
                prop_assert_eq!(decoded.encoded(), encoded);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn every_valid_contract_strkey_is_56_base32_chars_with_a_checksum() {
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        runner
            .run(&contract_strkey(), |encoded| {
                prop_assert_eq!(encoded.len(), 56);
                prop_assert!(encoded.starts_with('C'));
                let decoded = Strkey::decode(&encoded).expect("generated strkeys decode");
                prop_assert_eq!(decoded.kind(), StrkeyKind::Contract);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn corrupted_strkeys_are_rejected_by_the_parser() {
        let mut runner = TestRunner::new(ProptestConfig::with_cases(128));
        runner
            .run(&corrupted_strkey(), |encoded| {
                prop_assert_eq!(encoded.len(), 56);
                match Strkey::decode(&encoded) {
                    Err(StrkeyError::InvalidCharacter { .. })
                    | Err(StrkeyError::InvalidChecksum { .. }) => Ok(()),
                    other => Err(TestCaseError::fail(format!(
                        "a corrupted strkey decoded or failed for the wrong reason: {other:?}"
                    ))),
                }
            })
            .unwrap();
    }

    #[test]
    fn a_checksum_typo_is_reported_as_a_checksum_error() {
        let decoded = Strkey::decode(&zero_account_strkey()).unwrap();
        let corrupted = super::corrupt_checksum(&decoded);
        assert!(matches!(
            Strkey::decode(&corrupted),
            Err(StrkeyError::InvalidChecksum { .. })
        ));
    }

    #[test]
    fn wrong_lengths_and_foreign_characters_are_rejected_cleanly() {
        assert!(matches!(
            Strkey::decode("GABC"),
            Err(StrkeyError::InvalidLength { length: 4 })
        ));
        let mut lowered = zero_account_strkey();
        lowered.replace_range(10..11, "a");
        assert!(matches!(
            Strkey::decode(&lowered),
            Err(StrkeyError::InvalidCharacter { index: 10, .. })
        ));
    }

    #[test]
    fn an_unknown_version_byte_is_rejected_after_the_checksum_passes() {
        let bytes = super::encode_bytes(0x7f, &[0u8; 32]);
        assert!(matches!(
            Strkey::decode(&bytes),
            Err(StrkeyError::UnknownVersion(0x7f))
        ));
    }

    #[test]
    fn decoded_strkeys_round_trip_through_a_soroban_address() {
        let env = Env::default();
        let mut runner = TestRunner::new(ProptestConfig::with_cases(64));
        runner
            .run(&valid_strkey(), |encoded| {
                let decoded = Strkey::decode(&encoded).expect("valid strkey");
                let address = decoded.to_address(&env);
                let rebuilt = Strkey::decode(&decoded.encoded()).expect("re-encoded strkey");
                prop_assert_eq!(rebuilt.kind(), decoded.kind());
                prop_assert_eq!(rebuilt.as_bytes(), decoded.as_bytes());
                drop(address);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_failing_property_shrinks_to_the_canonical_zero_strkey() {
        let config = ProptestConfig {
            failure_persistence: None,
            // Each of the 32 payload bytes shrinks on its own, so the default
            // 128 iterations stop short of the all-zero payload.
            max_shrink_iters: 4_096,
            ..ProptestConfig::with_cases(32)
        };
        let mut runner = TestRunner::new(config);
        let outcome = runner.run(&account_strkey(), |_| {
            Err::<(), _>(TestCaseError::fail("always fails for shrinking"))
        });

        match outcome {
            Err(TestError::Fail(_, encoded)) => {
                assert_eq!(encoded, zero_account_strkey());
            }
            other => panic!("expected a shrunk counterexample, got {other:?}"),
        }
    }

    #[test]
    fn the_zero_strkeys_are_valid_and_distinct() {
        let account = zero_account_strkey();
        let contract = zero_contract_strkey();
        assert_ne!(account, contract);
        assert_eq!(
            Strkey::decode(&account).unwrap().kind(),
            StrkeyKind::Account
        );
        assert_eq!(
            Strkey::decode(&contract).unwrap().kind(),
            StrkeyKind::Contract
        );
        assert_eq!(Strkey::zero(StrkeyKind::Account).as_bytes(), &[0u8; 32]);
    }
}
