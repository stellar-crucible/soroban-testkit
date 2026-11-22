mod common;

use common::{Fixture, INTERVAL, START};
use proptest::prelude::*;
use soroban_testkit_example_subscription::SubscriptionError;
use soroban_testkit_generators::strategies::{account_address, contract_address, timestamp};

/// A recurring fee small enough that eight cycles stay inside `i128` and fast
/// enough that a case can bill several times.
fn recurring_fee() -> impl Strategy<Value = i128> {
    1i128..=100_000i128
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// The core invariant: over any sequence of billing attempts, the merchant
    /// receives the authorized fee once per elapsed interval and never more.
    #[test]
    fn a_merchant_never_withdraws_more_than_the_fee_per_cycle(
        fee in recurring_fee(),
        elapsed in 0u64..=(INTERVAL * 3),
        attempts in 1usize..=6,
    ) {
        let mut fixture = Fixture::with_limits(fee, INTERVAL, fee * 8, fee * 8, None);
        fixture.subscribe();
        fixture.advance_time(elapsed);

        let mut withdrawn = 0i128;
        let mut last_balance = fixture.merchant_balance();

        for _ in 0..attempts {
            match fixture.try_bill() {
                Ok(_) => {
                    let current = fixture.merchant_balance();
                    let delta = current - last_balance;
                    // A successful cycle moves exactly the authorized fee.
                    prop_assert_eq!(delta, fee);
                    withdrawn += delta;
                    last_balance = current;
                }
                Err(error) => {
                    // A rejected cycle moves nothing.
                    prop_assert_eq!(
                        error,
                        SubscriptionError::BillingNotDue,
                        "the only expected rejection inside the funded range"
                    );
                    prop_assert_eq!(fixture.merchant_balance(), last_balance);
                }
            }
        }

        let payable_cycles = (elapsed / INTERVAL) as i128;
        prop_assert!(withdrawn <= fee * payable_cycles);
        prop_assert_eq!(
            withdrawn,
            fee * fixture.subscription().unwrap().cycles_billed as i128
        );
        prop_assert_eq!(
            fixture.balance(&fixture.subscriber),
            fee * 8 - withdrawn
        );
    }

    #[test]
    fn calls_inside_one_cycle_never_withdraw(
        fee in recurring_fee(),
        elapsed in 0u64..INTERVAL,
        attempts in 1usize..=5,
    ) {
        let mut fixture = Fixture::with_limits(fee, INTERVAL, fee * 8, fee * 8, None);
        fixture.subscribe();
        fixture.advance_time(elapsed);

        for _ in 0..attempts {
            prop_assert_eq!(
                fixture.try_bill().unwrap_err(),
                SubscriptionError::BillingNotDue
            );
        }

        prop_assert_eq!(fixture.merchant_balance(), 0);
        prop_assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
        prop_assert_eq!(fixture.balance(&fixture.subscriber), fee * 8);
    }

    #[test]
    fn advancing_time_permits_one_charge_per_interval(
        fee in recurring_fee(),
        elapsed in 0u64..=(INTERVAL * 3),
        attempts in 1usize..=3,
    ) {
        let mut fixture = Fixture::with_limits(fee, INTERVAL, fee * 8, fee * 8, None);
        fixture.subscribe();
        fixture.advance_time(elapsed);

        let payable = (elapsed / INTERVAL) as usize;
        for attempt in 0..attempts {
            if attempt < payable {
                prop_assert!(fixture.try_bill().is_ok());
            } else {
                prop_assert_eq!(
                    fixture.try_bill().unwrap_err(),
                    SubscriptionError::BillingNotDue
                );
            }
        }

        let expected_cycles = payable.min(attempts) as u32;
        prop_assert_eq!(
            fixture.subscription().unwrap().cycles_billed,
            expected_cycles
        );
        prop_assert_eq!(
            fixture.merchant_balance(),
            fee * expected_cycles as i128
        );
    }

    #[test]
    fn cancellation_stops_every_future_withdrawal(
        fee in recurring_fee(),
        elapsed in 0u64..=(INTERVAL * 3),
    ) {
        let mut fixture = Fixture::with_limits(fee, INTERVAL, fee * 8, fee * 8, None);
        fixture.subscribe();
        fixture.cancel();
        fixture.advance_time(elapsed);

        prop_assert_eq!(
            fixture.try_bill().unwrap_err(),
            SubscriptionError::AlreadyCanceled
        );
        prop_assert_eq!(fixture.merchant_balance(), 0);
        prop_assert_eq!(fixture.balance(&fixture.subscriber), fee * 8);
    }

    #[test]
    fn an_unauthorized_caller_never_withdraws(
        fee in recurring_fee(),
        elapsed in 0u64..=(INTERVAL * 3),
    ) {
        let mut fixture = Fixture::with_limits(fee, INTERVAL, fee * 8, fee * 8, None);
        fixture.subscribe();
        fixture.advance_time(elapsed);

        prop_assert_eq!(
            fixture.try_bill_as(&fixture.outsider).unwrap_err(),
            SubscriptionError::Unauthorized
        );
        prop_assert_eq!(fixture.merchant_balance(), 0);
        prop_assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
    }

    #[test]
    fn a_charge_at_the_boundary_is_exact_across_ledger_times(
        fee in recurring_fee(),
        start in timestamp(),
    ) {
        let mut fixture = Fixture::with_limits(fee, INTERVAL, fee * 4, fee * 4, None);
        fixture.set_time(start);
        fixture.subscribe_at(start + INTERVAL);
        fixture.advance_time(INTERVAL);

        prop_assert_eq!(fixture.bill(), 1);
        prop_assert_eq!(fixture.merchant_balance(), fee);
        prop_assert_eq!(
            fixture.subscription().unwrap().next_billing,
            start + INTERVAL * 2
        );

        prop_assert_eq!(
            fixture.try_bill().unwrap_err(),
            SubscriptionError::BillingNotDue
        );
        prop_assert_eq!(fixture.merchant_balance(), fee);
    }

    #[test]
    fn contract_addresses_can_be_billed(
        fee in recurring_fee(),
        subscriber in contract_address(),
        merchant in contract_address(),
    ) {
        let mut fixture = Fixture::with_limits(
            fee,
            INTERVAL,
            fee * 4,
            fee * 4,
            Some((subscriber, merchant)),
        );
        fixture.subscribe();
        fixture.advance_time(INTERVAL);

        prop_assert_eq!(fixture.bill(), 1);
        prop_assert_eq!(fixture.merchant_balance(), fee);
        prop_assert_eq!(fixture.balance(&fixture.subscriber), fee * 3);
        prop_assert_eq!(
            fixture.subscription().unwrap().subscriber,
            fixture.subscriber.clone()
        );
    }

    #[test]
    fn account_addresses_can_hold_and_cancel_a_subscription(
        fee in recurring_fee(),
        subscriber in account_address(),
        merchant in contract_address(),
    ) {
        // Generated ed25519 accounts are not ledger accounts the Stellar asset
        // contract can mint to, so this case covers the identity, storage and
        // cancellation path that does not move tokens.
        let fixture = Fixture::without_funding(fee, INTERVAL, (subscriber, merchant));
        fixture.subscribe();

        let subscription = fixture.subscription().unwrap();
        prop_assert_eq!(subscription.subscriber, fixture.subscriber.clone());
        prop_assert_eq!(subscription.merchant, fixture.merchant.clone());
        prop_assert_eq!(subscription.fee, fee);
        prop_assert!(fixture.client().is_active(&fixture.subscriber));

        fixture.cancel();
        prop_assert!(fixture.subscription().unwrap().canceled);
        prop_assert!(!fixture.client().is_active(&fixture.subscriber));
    }
}

#[test]
fn the_default_schedule_starts_one_interval_after_the_fixture_clock() {
    // The generated cases above rely on this fixture boundary, so it is pinned
    // once outside proptest rather than assumed inside every case.
    let fixture = Fixture::new();
    fixture.subscribe();
    assert_eq!(
        fixture.subscription().unwrap().next_billing,
        START + INTERVAL
    );
}
