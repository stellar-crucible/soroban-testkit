mod common;

use common::{Fixture, FEE, INTERVAL, NEXT_BILLING, START};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::assert_balance_conserved;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_example_subscription::SubscriptionError;

#[test]
fn subscribe_requires_the_subscribers_authorization() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let subscriber = fixture.subscriber.clone();
    let merchant = fixture.merchant.clone();
    let token = fixture.token_id.clone();
    assert_auth_matches!(fixture.ctx.env, {
        invoker: subscriber,
        function: "subscribe",
        args: [_, merchant, token, FEE, INTERVAL, NEXT_BILLING],
    });

    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
}

#[test]
fn bill_requires_the_merchants_authorization() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);
    fixture.bill();

    let merchant = fixture.merchant.clone();
    assert_auth_matches!(fixture.ctx.env, {
        invoker: merchant,
        function: "bill",
    });

    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
}

#[test]
fn cancel_requires_the_subscribers_authorization() {
    let fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();

    let subscriber = fixture.subscriber.clone();
    assert_auth_matches!(fixture.ctx.env, {
        invoker: subscriber,
        function: "cancel",
    });
}

#[test]
fn subscribe_publishes_the_agreed_terms() {
    let fixture = Fixture::new();
    fixture.subscribe();

    // The payload is the record a merchant would read from the chain, so each
    // term is asserted rather than only the event's existence.
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("created")
        .assert_data_matches(|data| {
            data.field("fee")
                .and_then(|value| value.deserialize::<i128>())
                == Some(FEE)
                && data
                    .field("interval")
                    .and_then(|value| value.deserialize::<u64>())
                    == Some(INTERVAL)
        });
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("created")
        .assert_data_matches(|data| {
            data.field("merchant")
                .and_then(|value| value.deserialize::<soroban_sdk::Address>())
                == Some(fixture.merchant.clone())
        });
}

#[test]
fn bill_publishes_the_amount_and_the_next_cycle() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);
    fixture.bill();

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("billed")
        .assert_data_matches(|data| {
            data.field("amount")
                .and_then(|value| value.deserialize::<i128>())
                == Some(FEE)
                && data
                    .field("next_billing")
                    .and_then(|value| value.deserialize::<u64>())
                    == Some(START + INTERVAL * 2)
        });
}

#[test]
fn cancel_publishes_the_cancellation() {
    let fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();

    EventMatcher::new(&fixture.ctx.env)
        .with_topic("canceled")
        .assert_data_matches(|data| {
            data.field("subscriber")
                .and_then(|value| value.deserialize::<soroban_sdk::Address>())
                == Some(fixture.subscriber.clone())
        });
}

#[test]
fn initial_billing_moves_exactly_the_recurring_fee() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let subscriber_before = fixture.balance(&fixture.subscriber);
    let cycles = fixture.bill();

    assert_eq!(cycles, 1);
    assert_eq!(fixture.merchant_balance(), FEE);
    assert_eq!(
        fixture.balance(&fixture.subscriber),
        subscriber_before - FEE
    );
    assert_eq!(fixture.allowance_left(), fixture.fee * 4 - FEE);
}

#[test]
fn initial_billing_schedules_the_following_cycle() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);
    fixture.bill();

    let subscription = fixture.subscription().unwrap();
    assert_eq!(subscription.cycles_billed, 1);
    assert_eq!(subscription.next_billing, START + INTERVAL * 2);
    assert!(!fixture.client().is_billing_due(&fixture.subscriber));
}

#[test]
fn recurring_billing_charges_once_per_interval() {
    let mut fixture = Fixture::new();
    fixture.subscribe();

    for cycle in 1..=3 {
        fixture.advance_time(INTERVAL);
        assert_eq!(fixture.bill(), cycle);
        assert_eq!(fixture.merchant_balance(), FEE * cycle as i128);
        assert_eq!(
            fixture.subscription().unwrap().next_billing,
            START + INTERVAL * (cycle as u64 + 1)
        );
    }
}

#[test]
fn billing_before_the_interval_elapses_is_rejected() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL - 1);

    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::BillingNotDue);
    assert_eq!(fixture.merchant_balance(), 0);
    assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
}

#[test]
fn billing_is_accepted_at_the_exact_boundary_second() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    assert_eq!(fixture.now(), NEXT_BILLING);
    assert_eq!(fixture.bill(), 1);
    assert_eq!(fixture.merchant_balance(), FEE);
}

#[test]
fn repeated_billing_in_the_same_cycle_is_rejected() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);
    fixture.bill();

    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::BillingNotDue);
    assert_eq!(fixture.merchant_balance(), FEE);
    assert_eq!(fixture.subscription().unwrap().cycles_billed, 1);
}

#[test]
fn a_gap_allows_one_charge_per_elapsed_interval() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL * 3);

    assert_eq!(fixture.bill(), 1);
    assert_eq!(fixture.bill(), 2);
    assert_eq!(fixture.bill(), 3);
    assert_eq!(fixture.merchant_balance(), FEE * 3);

    // The fourth call has caught up with the clock, so the next cycle is not
    // payable until another interval passes. One call never charges two cycles.
    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::BillingNotDue);
    assert_eq!(fixture.merchant_balance(), FEE * 3);
}

#[test]
fn billing_after_cancellation_is_rejected() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();
    fixture.advance_time(INTERVAL);

    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::AlreadyCanceled);
    assert_eq!(fixture.merchant_balance(), 0);
    assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
}

#[test]
fn cancellation_preserves_the_schedule_and_balance() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);
    fixture.bill();
    let next_billing = fixture.subscription().unwrap().next_billing;
    let merchant_balance = fixture.merchant_balance();

    fixture.cancel();

    assert_eq!(fixture.subscription().unwrap().next_billing, next_billing);
    assert_eq!(fixture.merchant_balance(), merchant_balance);
    fixture.advance_time(INTERVAL);
    assert_eq!(
        fixture.try_bill().unwrap_err(),
        SubscriptionError::AlreadyCanceled
    );
}

#[test]
fn a_merchant_cannot_cancel_a_subscription() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let result = fixture.try_cancel_as(&fixture.merchant);
    assert_eq!(result.unwrap_err(), SubscriptionError::Unauthorized);
    assert!(!fixture.subscription().unwrap().canceled);
}

#[test]
fn an_outsider_cannot_cancel_a_subscription() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let result = fixture.try_cancel_as(&fixture.outsider);
    assert_eq!(result.unwrap_err(), SubscriptionError::Unauthorized);
    assert!(fixture.client().is_active(&fixture.subscriber));
}

#[test]
fn an_outsider_cannot_bill_a_subscription() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let result = fixture.try_bill_as(&fixture.outsider);
    assert_eq!(result.unwrap_err(), SubscriptionError::Unauthorized);
    assert_eq!(fixture.merchant_balance(), 0);
    assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
}

#[test]
fn canceling_twice_is_rejected() {
    let fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();

    assert_eq!(
        fixture.try_cancel().unwrap_err(),
        SubscriptionError::AlreadyCanceled
    );
}

#[test]
fn canceling_an_unknown_subscription_is_rejected() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.try_cancel().unwrap_err(),
        SubscriptionError::SubscriptionNotFound
    );
}

#[test]
fn billing_an_unknown_subscription_is_rejected() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.try_bill().unwrap_err(),
        SubscriptionError::SubscriptionNotFound
    );
}

#[test]
fn an_insufficient_allowance_blocks_the_charge_and_keeps_the_schedule() {
    let mut fixture = Fixture::with_limits(FEE, INTERVAL, 10_000, FEE - 1, None);
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::PaymentFailed);
    assert_eq!(fixture.merchant_balance(), 0);
    assert_eq!(fixture.balance(&fixture.subscriber), 10_000);
    assert_eq!(fixture.allowance_left(), FEE - 1);

    let subscription = fixture.subscription().unwrap();
    assert_eq!(subscription.next_billing, NEXT_BILLING);
    assert_eq!(subscription.cycles_billed, 0);
}

#[test]
fn an_insufficient_balance_blocks_the_charge() {
    let mut fixture = Fixture::with_limits(FEE, INTERVAL, FEE - 1, 10_000, None);
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::PaymentFailed);
    assert_eq!(fixture.merchant_balance(), 0);
    assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
}

#[test]
fn a_rejected_payment_publishes_no_billing_event() {
    let mut fixture = Fixture::with_limits(FEE, INTERVAL, 10_000, 0, None);
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    assert_eq!(
        fixture.try_bill().unwrap_err(),
        SubscriptionError::PaymentFailed
    );
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("billed")
        .assert_not_emitted();
}

#[test]
fn a_billing_cycle_conserves_the_token_supply() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    assert_balance_conserved!(
        accounts: [
            (&fixture.subscriber, "subscriber"),
            (&fixture.merchant, "merchant")
        ],
        balance: |address| fixture.balance(address),
        call: {
            fixture.bill();
        }
    );
}

#[test]
fn the_next_billing_time_overflows_safely_at_the_end_of_the_clock() {
    let mut fixture = Fixture::new();
    fixture.set_time(u64::MAX - 1);
    fixture.subscribe_at(u64::MAX);
    fixture.advance_time(1);

    let result = fixture.try_bill();
    assert_eq!(result.unwrap_err(), SubscriptionError::TimestampOverflow);
    assert_eq!(fixture.merchant_balance(), 0);
    assert_eq!(fixture.subscription().unwrap().cycles_billed, 0);
}
