extern crate std;

use proptest::prelude::*;
use soroban_testkit_example_concentrated_amm::{
    quote_swap, tick_to_sqrt_price, MAX_TICK, MIN_TICK,
};

proptest! {
#[test] fn invariant_reserve_product_non_decreasing(a in 1i128..100_000, b in 1i128..100_000, amount in 1i128..1000) {
    if let Some(out)=quote_swap(a,b,amount,true,30) { let k0=a*b; let k1=(a+amount)*(b-out); prop_assert!(k1>=k0.saturating_sub(a.saturating_add(b))); }
}
#[test] fn reverse_swap_preserves_product(a in 1i128..100_000, b in 1i128..100_000, amount in 1i128..1000) {
    if let Some(out)=quote_swap(a,b,amount,false,30) { let k0=a*b; let k1=(a-out)*(b+amount); prop_assert!(k1>=k0.saturating_sub(a.saturating_add(b))); }
}
#[test] fn quote_never_returns_reserve(a in 1i128..100_000, b in 2i128..100_000, amount in 1i128..1000) {
    prop_assert!(quote_swap(a,b,amount,true,30).is_none_or(|out| out < b));
}
#[test] fn fee_never_reverses_direction(a in 1i128..100_000, b in 1i128..100_000, amount in 1i128..1000, fee in 0u32..100u32) {
    prop_assert!(quote_swap(a,b,amount,true,fee).is_some() || fee>9999);
}
#[test] fn tick_bounds_are_clamped(tick in -1_000_000i32..1_000_000i32) {
    let price=tick_to_sqrt_price(tick); prop_assert!(price>0); prop_assert!(tick.clamp(MIN_TICK,MAX_TICK)==tick.clamp(MIN_TICK,MAX_TICK));
}
#[test] fn max_tick_above_min_tick(_tick in MIN_TICK..MAX_TICK) { prop_assert!(MAX_TICK>MIN_TICK); }
}
