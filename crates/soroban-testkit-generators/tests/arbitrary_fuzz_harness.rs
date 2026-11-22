//! Runs the cargo-fuzz harness on stable.
//!
//! `fuzz/fuzz_targets/harness.rs` is the body of the libFuzzer target. This
//! test drives it with a deterministic corpus — empty input, short inputs,
//! saturated inputs and pseudo-random buffers — so the "generation never panics
//! and always yields a valid value" property is checked on every CI run, not
//! only when someone runs `cargo +nightly fuzz`.

#[path = "../fuzz/fuzz_targets/harness.rs"]
mod harness;

fn xorshift_bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

#[test]
fn harness_accepts_empty_and_short_inputs() {
    for len in 0..64 {
        harness::check(&vec![0u8; len]);
    }
}

#[test]
fn harness_accepts_saturated_inputs() {
    for len in [1, 16, 64, 128, 512] {
        harness::check(&vec![0xFF; len]);
    }
}

#[test]
fn harness_accepts_a_pseudo_random_corpus() {
    for seed in 0..256 {
        let len = (seed as usize * 7) % 200;
        harness::check(&xorshift_bytes(seed, len));
    }
}
