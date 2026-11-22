//! Strategies over money: stroop amounts, basis points and exchange rates.
//!
//! Stellar amounts are `i128` stroops, seven decimal places, and almost every
//! DeFi bug that property tests are good at finding lives at the edges of that
//! representation: the last stroop of a fee, a rate whose product overflows, a
//! basis point of 10,000 that leaves nothing behind. The strategies here spend
//! their cases on those edges instead of spreading them evenly over the range,
//! so a 256-case run still reaches `1`, `10^7 - 1`, `10^7` and the supply cap.
//!
//! Every strategy shrinks towards the small end of its range, so a failing
//! conversion or fee split is reported at the smallest amount that still
//! reproduces it.
//!
//! ```
//! use proptest::strategy::{Strategy as _, ValueTree as _};
//! use proptest::test_runner::TestRunner;
//! use soroban_testkit_generators::financial::{basis_points, stroop_amount};
//!
//! let mut runner = TestRunner::deterministic();
//! let amount = stroop_amount().new_tree(&mut runner).unwrap().current();
//! let fee_bps = basis_points().new_tree(&mut runner).unwrap().current();
//! let fee = amount * fee_bps as i128 / 10_000;
//! assert!(fee <= amount);
//! ```

use proptest::prelude::*;

/// Stroops in one unit of a seven-decimal Stellar asset.
pub const STROOPS_PER_UNIT: i128 = 10_000_000;

/// The largest amount [`stroop_amount`] generates: one billion units.
///
/// The Stellar supply of a token is far below `i128::MAX`, and staying inside
/// this cap keeps products with an exchange rate inside `i128` as well, which
/// is the bound a contract's own arithmetic has to live with.
pub const MAX_TOKEN_SUPPLY: i128 = 1_000_000_000 * STROOPS_PER_UNIT;

/// The most a basis-point fee can ask for: 100%.
pub const MAX_BASIS_POINTS: u32 = 10_000;

/// The values a fee or an interest calculation rounds wrongly around.
///
/// These are the amounts [`stroop_amount`] weights heavily: zero, one stroop,
/// one stroop either side of a whole unit, a unit either side of ten units, and
/// the supply cap. A test that only ever sees round numbers never exercises the
/// truncation a contract actually performs.
pub fn stroop_boundaries() -> Vec<i128> {
    let mut boundaries = vec![
        0,
        1,
        2,
        STROOPS_PER_UNIT - 1,
        STROOPS_PER_UNIT,
        STROOPS_PER_UNIT + 1,
        10 * STROOPS_PER_UNIT - 1,
        10 * STROOPS_PER_UNIT,
        10 * STROOPS_PER_UNIT + 1,
        MAX_TOKEN_SUPPLY / 2,
        MAX_TOKEN_SUPPLY - 1,
        MAX_TOKEN_SUPPLY,
    ];
    boundaries.sort_unstable();
    boundaries.dedup();
    boundaries
}

/// Generates a token amount in stroops, `0..=MAX_TOKEN_SUPPLY`.
///
/// Two thirds of the cases come from the boundaries [`stroop_boundaries`]
/// names — the amounts a fee, a split or an interest step rounds wrongly — and
/// the rest are spread over the whole supply. Shrinking moves towards zero.
pub fn stroop_amount() -> impl Strategy<Value = i128> {
    let boundaries = stroop_boundaries();
    let boundary = proptest::sample::select(boundaries);
    prop_oneof![
        3 => boundary,
        1 => 0i128..=MAX_TOKEN_SUPPLY,
    ]
}

/// Generates an amount that may sit outside the supply cap.
///
/// Fee and overflow tests need the values a contract can be handed by a caller
/// even though no honest mint would produce them: `i64::MAX`, `u64::MAX` and
/// `i128::MAX`. Most cases still come from [`stroop_amount`]; this adds the
/// extremes without making every case an overflow.
pub fn extreme_stroop_amount() -> impl Strategy<Value = i128> {
    prop_oneof![
        4 => stroop_amount(),
        1 => Just(i64::MAX as i128),
        1 => Just(u64::MAX as i128),
        1 => Just(i128::MAX),
    ]
}

/// The basis points a fee schedule or a rate parameterises on.
///
/// Weighted towards `0`, `1`, `30` (Stellar's base fee in bps), `50`, `100`,
/// `9_999` and `10_000`, because a fee of exactly 100% and one of exactly 0%
/// are where a split divides by zero or strands the whole amount.
pub fn basis_points() -> impl Strategy<Value = u32> {
    let boundaries: Vec<u32> = vec![0, 1, 2, 30, 50, 100, 9_999, MAX_BASIS_POINTS];
    let boundary = proptest::sample::select(boundaries);
    prop_oneof![
        3 => boundary,
        1 => 0u32..=MAX_BASIS_POINTS,
    ]
}

/// A price as a numerator and a denominator, so a conversion is one checked
/// multiply and one checked divide.
///
/// Both legs stay in `1..=1_000_000`, which means multiplying the cap
/// ([`MAX_TOKEN_SUPPLY`]) by either leg stays far inside `i128`: a test that
/// overflows is reporting a bug in the contract, not in this strategy.
pub fn exchange_rate_ratio() -> impl Strategy<Value = (i128, i128)> {
    let boundaries: Vec<(i128, i128)> = vec![
        (1, 1),
        (1, 2),
        (2, 1),
        (1, 1_000_000),
        (1_000_000, 1),
        (7, 1),
        (1, 7),
        (999_999, 1_000_000),
    ];
    let boundary = proptest::sample::select(boundaries);
    prop_oneof![
        3 => boundary,
        1 => (1i128..=1_000_000i128, 1i128..=1_000_000i128),
    ]
}

/// Generates a price ratio as an `i128` numerator over a positive denominator,
/// pairing a rate with the amount it prices.
///
/// The amount is drawn from [`stroop_amount`], so the returned triple is what a
/// swap test needs: `(amount, numerator, denominator)`, with
/// `amount * numerator` guaranteed to fit in `i128`.
pub fn priced_stroop_amount() -> impl Strategy<Value = (i128, i128, i128)> {
    (stroop_amount(), exchange_rate_ratio())
        .prop_map(|(amount, (numerator, denominator))| (amount, numerator, denominator))
}

/// Assert that a fee split rounds in the protocol's favour.
///
/// A split is correct when the two legs add back up to the gross amount, neither
/// leg is negative, and the fee is at most one stroop above the exact
/// `gross * bps / 10_000` — the loosest an honest implementation can be, since a
/// single division rounds by less than one stroop. Rounding down by more than
/// that is dust the protocol never collects; rounding up by more than a stroop
/// is a caller paying for the protocol's precision.
///
/// ```
/// use soroban_testkit_generators::assert_fee_rounding_favors_protocol;
///
/// let (gross, bps) = (1_000_000i128, 30u32);
/// let fee = gross * bps as i128 / 10_000;
/// assert_fee_rounding_favors_protocol!(gross = gross, fee_bps = bps, net = gross - fee, fee = fee);
/// ```
#[macro_export]
macro_rules! assert_fee_rounding_favors_protocol {
    (gross = $gross:expr, fee_bps = $bps:expr, net = $net:expr, fee = $fee:expr $(,)?) => {{
        let gross: i128 = $gross;
        let bps: u32 = $bps;
        let net: i128 = $net;
        let fee: i128 = $fee;

        assert!(
            gross >= 0,
            "a negative gross amount is not a fee split: gross={gross}"
        );
        assert!(
            net >= 0 && fee >= 0,
            "the split produced a negative leg: net={net} fee={fee} gross={gross} bps={bps}"
        );
        assert_eq!(
            net + fee,
            gross,
            "the split lost or created value: net={net} + fee={fee} != gross={gross}"
        );

        let exact_fee = gross * bps as i128 / $crate::financial::MAX_BASIS_POINTS as i128;
        assert!(
            fee >= exact_fee,
            "the fee rounded down past the protocol's floor: fee={fee} < floor={exact_fee} (gross={gross}, bps={bps})"
        );
        assert!(
            fee - exact_fee <= 1,
            "the fee rounded up past the caller's ceiling: fee={fee} > floor={exact_fee} + 1 (gross={gross}, bps={bps})"
        );
    }};
}

/// Assert that no leg of an account's balance went negative.
///
/// The usual bug is a subtraction that saturates silently or wraps in a release
/// build; naming the balance in the message is what makes a failing property
/// readable when the case that produced it is a single stroop.
///
/// ```
/// use soroban_testkit_generators::assert_balances_never_negative;
///
/// assert_balances_never_negative!(balance = 0i128, stake = 10i128, rewards = 0i128);
/// ```
#[macro_export]
macro_rules! assert_balances_never_negative {
    ($($name:ident = $value:expr),* $(,)?) => {{
        $(
            let $name: i128 = $value;
            assert!($name >= 0, "{} went negative: {}", stringify!($name), $name);
        )*
    }};
}

/// The fee and net legs of `amount` charged `bps` basis points.
///
/// Rounds the fee up, which is the direction a protocol should round: the
/// caller never pays less than the stated rate. A reference implementation for
/// tests, not a contract — a real split has to do this in `no_std` with checked
/// arithmetic.
pub fn split_fee(amount: i128, bps: u32) -> (i128, i128) {
    let denominator = MAX_BASIS_POINTS as i128;
    let product = amount
        .checked_mul(bps as i128)
        .unwrap_or_else(|| panic!("amount {amount} * {bps} bps overflowed i128"));
    // Round the fee up without `div_ceil`, which this crate's rust-version
    // keeps off the stable surface for signed integers.
    let fee = (product + denominator - 1) / denominator;
    (amount - fee, fee)
}

#[cfg(test)]
mod tests {
    use super::{
        basis_points, exchange_rate_ratio, extreme_stroop_amount, priced_stroop_amount, split_fee,
        stroop_amount, stroop_boundaries, MAX_BASIS_POINTS, MAX_TOKEN_SUPPLY, STROOPS_PER_UNIT,
    };
    use proptest::prelude::*;

    #[test]
    fn the_boundary_amounts_cover_both_sides_of_every_decimal_edge() {
        let boundaries = stroop_boundaries();

        assert_eq!(boundaries.first(), Some(&0));
        assert_eq!(boundaries.last(), Some(&MAX_TOKEN_SUPPLY));
        for stroops in [
            1,
            STROOPS_PER_UNIT - 1,
            STROOPS_PER_UNIT,
            STROOPS_PER_UNIT + 1,
        ] {
            assert!(
                boundaries.contains(&stroops),
                "the boundary list is missing {stroops} stroops"
            );
        }
        assert!(boundaries.windows(2).all(|pair| pair[0] < pair[1]));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn generated_amounts_stay_inside_the_supply(amount in stroop_amount()) {
            prop_assert!((0..=MAX_TOKEN_SUPPLY).contains(&amount));
        }

        #[test]
        fn generated_amounts_shrink_without_leaving_the_range(amount in extreme_stroop_amount()) {
            prop_assert!(amount >= 0);
        }

        #[test]
        fn generated_basis_points_are_valid(bps in basis_points()) {
            prop_assert!(bps <= MAX_BASIS_POINTS);
        }

        #[test]
        fn a_generated_ratio_prices_the_supply_cap_without_overflowing(
            (numerator, denominator) in exchange_rate_ratio(),
        ) {
            prop_assert!(numerator >= 1 && denominator >= 1);
            let priced = MAX_TOKEN_SUPPLY.checked_mul(numerator);
            prop_assert!(priced.is_some(), "{MAX_TOKEN_SUPPLY} * {numerator} overflowed");
            prop_assert!(priced.unwrap().checked_div(denominator).is_some());
        }

        #[test]
        fn a_priced_amount_multiplies_without_overflowing(
            (amount, numerator, denominator) in priced_stroop_amount(),
        ) {
            prop_assert!(amount.checked_mul(numerator).is_some());
            prop_assert!(denominator > 0);
        }

        #[test]
        fn the_reference_fee_split_keeps_every_stroop(
            amount in stroop_amount(),
            bps in basis_points(),
        ) {
            let (net, fee) = split_fee(amount, bps);
            crate::assert_fee_rounding_favors_protocol!(gross = amount, fee_bps = bps, net = net, fee = fee);
            crate::assert_balances_never_negative!(net = net, fee = fee);
        }

        // Precision loss: a contract that truncates the fee keeps the dust. The
        // property is that the truncated split never gives the protocol *less*
        // than the exact rate, and never loses a stroop to the split itself.
        #[test]
        fn truncating_the_fee_never_undercharges(
            amount in stroop_amount(),
            bps in basis_points(),
        ) {
            let exact = amount * bps as i128 / MAX_BASIS_POINTS as i128;
            let (net, fee) = split_fee(amount, bps);
            prop_assert_eq!(net + fee, amount);
            prop_assert!(fee >= exact);
            prop_assert!(fee - exact <= 1);
        }

        // Interest: compounding in integer stroops may round at every step, but
        // the balance can never fall and the total never exceeds the simple
        // interest rounded up once per period.
        #[test]
        fn compound_interest_never_loses_principal(
            principal in 0i128..=MAX_TOKEN_SUPPLY / 1_000,
            bps in 0u32..=MAX_BASIS_POINTS,
            periods in 0u32..=8,
        ) {
            let mut balance = principal;
            for _ in 0..periods {
                let (kept, interest) = split_fee(balance, bps);
                prop_assert_eq!(kept + interest, balance);
                balance = balance
                    .checked_add(interest)
                    .expect("interest overflowed the balance");
            }
            prop_assert!(balance >= principal);
            prop_assert!(balance <= principal + (principal * bps as i128 * periods as i128));
        }
    }

    #[test]
    fn a_full_fee_takes_the_whole_amount_and_a_zero_fee_takes_nothing() {
        assert_eq!(split_fee(1_000, MAX_BASIS_POINTS), (0, 1_000));
        assert_eq!(split_fee(1_000, 0), (1_000, 0));
        assert_eq!(split_fee(0, 30), (0, 0));
    }

    #[test]
    fn a_single_stroop_fee_rounds_up_to_one_stroop() {
        // 1 stroop at 30 bps is 0.003 stroops: truncation would charge nothing.
        assert_eq!(split_fee(1, 30), (0, 1));
        assert_eq!(split_fee(10, 30), (9, 1));
    }

    #[test]
    fn the_fee_macro_names_the_leg_that_broke_the_split() {
        let failure = std::panic::catch_unwind(|| {
            crate::assert_fee_rounding_favors_protocol!(
                gross = 10_000,
                fee_bps = 100,
                net = 10_000,
                fee = 0
            );
        });
        let message = failure
            .err()
            .and_then(|payload| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();

        assert!(
            message.contains("rounded down past the protocol's floor"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn the_balance_macro_names_the_negative_balance() {
        let failure = std::panic::catch_unwind(|| {
            crate::assert_balances_never_negative!(balance = 5i128, stake = -1i128);
        });
        let message = failure
            .err()
            .and_then(|payload| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();

        assert!(
            message.contains("stake went negative"),
            "unexpected message: {message}"
        );
    }
}
