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

#[cfg(test)]
mod tests {
    use super::{ledger_sequence, timestamp, token_amount};
    use proptest::prelude::*;
    use proptest::test_runner::TestRunner;

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
