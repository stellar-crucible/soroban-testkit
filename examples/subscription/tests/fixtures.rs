mod common;

use common::{Fixture, INTERVAL, NEXT_BILLING, START};
use soroban_testkit_example_subscription::SubscriptionError;

#[test]
fn subscribe_records_every_term_of_the_agreement() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let subscription = fixture.subscription().expect("subscription was stored");
    assert_eq!(subscription.subscriber, fixture.subscriber);
    assert_eq!(subscription.merchant, fixture.merchant);
    assert_eq!(subscription.token, fixture.token_id);
    assert_eq!(subscription.fee, fixture.fee);
    assert_eq!(subscription.interval, fixture.interval);
    assert_eq!(subscription.next_billing, NEXT_BILLING);
}

#[test]
fn subscribe_starts_active_with_no_cycles_billed() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let subscription = fixture.subscription().unwrap();
    assert!(!subscription.canceled);
    assert_eq!(subscription.cycles_billed, 0);
    assert!(fixture.client().is_active(&fixture.subscriber));
}

#[test]
fn subscribe_rejects_a_zero_fee() {
    let fixture = Fixture::new();
    let result = fixture.try_subscribe(&fixture.subscriber, 0, INTERVAL, NEXT_BILLING);
    assert_eq!(result.unwrap_err(), SubscriptionError::InvalidFee);
    assert!(fixture.subscription().is_none());
}

#[test]
fn subscribe_rejects_a_negative_fee() {
    let fixture = Fixture::new();
    let result = fixture.try_subscribe(&fixture.subscriber, -1, INTERVAL, NEXT_BILLING);
    assert_eq!(result.unwrap_err(), SubscriptionError::InvalidFee);
}

#[test]
fn subscribe_rejects_a_zero_interval() {
    let fixture = Fixture::new();
    let result = fixture.try_subscribe(&fixture.subscriber, 100, 0, NEXT_BILLING);
    assert_eq!(result.unwrap_err(), SubscriptionError::InvalidInterval);
}

#[test]
fn subscribe_rejects_a_billing_time_in_the_past() {
    let fixture = Fixture::new();
    let result = fixture.try_subscribe(&fixture.subscriber, 100, INTERVAL, START - 1);
    assert_eq!(result.unwrap_err(), SubscriptionError::InvalidSchedule);
}

#[test]
fn subscribe_accepts_an_immediate_first_cycle() {
    let fixture = Fixture::new();
    fixture.subscribe_at(START);

    let subscription = fixture.subscription().unwrap();
    assert_eq!(subscription.next_billing, START);
    assert!(fixture.client().is_billing_due(&fixture.subscriber));
}

#[test]
fn subscribe_rejects_a_second_active_subscription() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let result = fixture.try_subscribe(&fixture.subscriber, 100, INTERVAL, NEXT_BILLING + INTERVAL);
    assert_eq!(result.unwrap_err(), SubscriptionError::AlreadyActive);
}

#[test]
fn subscribe_replaces_a_canceled_subscription() {
    let fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();

    fixture
        .try_subscribe(&fixture.subscriber, 900, INTERVAL, NEXT_BILLING)
        .expect("a canceled agreement can be replaced");

    let subscription = fixture.subscription().unwrap();
    assert_eq!(subscription.fee, 900);
    assert!(!subscription.canceled);
    assert_eq!(subscription.cycles_billed, 0);
}

#[test]
fn cancel_marks_the_subscription_canceled() {
    let fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();

    let subscription = fixture.subscription().unwrap();
    assert!(subscription.canceled);
    assert!(!fixture.client().is_active(&fixture.subscriber));
}

#[test]
fn subscription_getter_returns_none_for_an_unknown_subscriber() {
    let fixture = Fixture::new();
    assert!(fixture.subscription().is_none());

    let stranger = fixture.outsider.clone();
    assert!(fixture.client().subscription(&stranger).is_none());
    assert!(!fixture.client().is_active(&stranger));
    assert!(!fixture.client().is_billing_due(&stranger));
}

#[test]
fn is_billing_due_follows_the_interval() {
    let mut fixture = Fixture::new();
    fixture.subscribe();

    assert!(!fixture.client().is_billing_due(&fixture.subscriber));
    fixture.advance_time(INTERVAL - 1);
    assert!(!fixture.client().is_billing_due(&fixture.subscriber));
    fixture.advance_time(1);
    assert!(fixture.client().is_billing_due(&fixture.subscriber));
}

#[test]
fn a_canceled_subscription_is_never_billing_due() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.cancel();
    fixture.advance_time(INTERVAL * 2);

    assert!(!fixture.client().is_billing_due(&fixture.subscriber));
}
