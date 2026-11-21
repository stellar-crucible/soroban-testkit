//! The property every `Arbitrary` implementation in
//! `soroban_testkit_generators::arbitrary` must hold: whatever bytes the
//! fuzzer supplies, generation never panics and every value is valid.
//!
//! Shared by the libFuzzer target (`fuzz_targets/arbitrary_values.rs`) and the
//! stable `tests/arbitrary_fuzz_harness.rs` integration test.

use arbitrary::{Arbitrary, Unstructured};
use soroban_sdk::xdr::ScAddress;
use soroban_sdk::Env;
use soroban_testkit_generators::arbitrary::{
    AddressBytes, AddressType, ArbitraryLedgerInfo, LedgerSequence, Timestamp, TokenAmount,
    LEDGER_SEQUENCE_MAX, LEDGER_SEQUENCE_MIN, PROTOCOL_VERSION, TIMESTAMP_MAX, TIMESTAMP_MIN,
    TOKEN_AMOUNT_MAX,
};

/// Values one fuzz input decodes into.
#[derive(Debug)]
pub struct Input {
    pub address: AddressBytes,
    pub amount: TokenAmount,
    pub sequence: LedgerSequence,
    pub timestamp: Timestamp,
    pub ledger: ArbitraryLedgerInfo,
}

impl<'a> Arbitrary<'a> for Input {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(Self {
            address: AddressBytes::arbitrary(u)?,
            amount: TokenAmount::arbitrary(u)?,
            sequence: LedgerSequence::arbitrary(u)?,
            timestamp: Timestamp::arbitrary(u)?,
            ledger: ArbitraryLedgerInfo::arbitrary(u)?,
        })
    }
}

/// Decode `data` and check every invariant. Panics on a violation.
pub fn check(data: &[u8]) {
    let mut u = Unstructured::new(data);
    // Every implementation degrades to the low end of its range when the
    // input runs out, so decoding itself must never fail.
    let input = Input::arbitrary(&mut u).expect("generation never fails");

    assert!((0..=TOKEN_AMOUNT_MAX).contains(&input.amount.get()));
    assert!((LEDGER_SEQUENCE_MIN..=LEDGER_SEQUENCE_MAX).contains(&input.sequence.get()));
    assert!((TIMESTAMP_MIN..=TIMESTAMP_MAX).contains(&input.timestamp.get()));

    let info = input.ledger.get();
    assert_eq!(info.protocol_version, PROTOCOL_VERSION);
    assert!(info.min_temp_entry_ttl >= 1);
    assert!(info.min_persistent_entry_ttl >= 1);
    assert!(info.max_entry_ttl >= info.min_temp_entry_ttl);
    assert!(info.max_entry_ttl >= info.min_persistent_entry_ttl);
    assert!(info
        .sequence_number
        .checked_add(info.max_entry_ttl)
        .is_some());

    let env = Env::default();
    input.ledger.apply(&env);
    assert_eq!(env.ledger().sequence(), info.sequence_number);
    assert_eq!(env.ledger().timestamp(), info.timestamp);

    let address = input.address.to_address(&env);
    let scaddress = ScAddress::from(&address);
    match input.address.address_type() {
        AddressType::Account => assert!(matches!(scaddress, ScAddress::Account(_))),
        AddressType::Contract => assert!(matches!(scaddress, ScAddress::Contract(_))),
    }
}
