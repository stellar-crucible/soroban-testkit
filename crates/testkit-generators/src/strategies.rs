use proptest::prelude::*;

pub fn token_amount() -> impl Strategy<Value = i128> {
    0i128..=1_000_000_000_000_000i128
}

pub fn ledger_sequence() -> impl Strategy<Value = u32> {
    1u32..=10_000_000u32
}

pub fn timestamp() -> impl Strategy<Value = u64> {
    1_600_000_000u64..=2_000_000_000u64
}
