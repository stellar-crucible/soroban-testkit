mod common;

use common::{Fixture, FEE, INTERVAL, NEXT_BILLING};
use soroban_testkit_assert::assert_storage_tier_count;
use soroban_testkit_core::assert_storage_tier;
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};
use soroban_testkit_core::namespace_auditor::NamespaceAuditor;
use soroban_testkit_core::storage::StorageTier;

fn snapshot(fixture: &Fixture) -> soroban_testkit_core::storage::StorageSnapshot {
    soroban_testkit_core::storage::StorageSnapshot::capture(&fixture.ctx.env, &fixture.contract_id)
}

#[test]
fn subscribe_consumes_cpu_and_memory() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let cost = BudgetSnapshot::last_invocation(&fixture.ctx.env);
    assert!(
        cost.cpu_insns > 0 && cost.mem_bytes > 0,
        "subscribe should be metered: {cost:?}"
    );
}

#[test]
fn billing_stays_inside_its_transaction_ceiling() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let guard = BudgetGuard::new("bill")
        .cpu_ceiling(5_000_000)
        .mem_ceiling(1_000_000);

    let cycles = guard.run(&fixture.ctx.env, || fixture.bill());

    assert_eq!(cycles, 1);
}

#[test]
fn billing_costs_more_than_reading_the_schedule() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);
    fixture.bill();
    let billing = BudgetSnapshot::last_invocation(&fixture.ctx.env);

    // A read parses one entry and transfers nothing, so it must undercut the
    // cycle that also moves tokens and rewrites the schedule.
    fixture.client().is_billing_due(&fixture.subscriber);
    let read = BudgetSnapshot::last_invocation(&fixture.ctx.env);

    assert!(
        read.cpu_insns < billing.cpu_insns,
        "a read should cost less than a billing cycle: {} vs {}",
        read.cpu_insns,
        billing.cpu_insns
    );
    assert!(read.diff(&billing).cpu_insns > 0);
}

#[test]
fn subscription_lives_in_persistent_storage() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture);
    assert!(before.is_empty());

    fixture.subscribe();

    assert_storage_tier_count!(fixture.ctx.env, fixture.contract_id, {
        instance: 0,
        persistent: 1,
        temporary: 0,
    });

    let after = snapshot(&fixture);
    let added = after.entries().first().expect("subscription entry added");
    assert_eq!(added.tier, StorageTier::Persistent);
    assert!(added.key.contains("Subscription"));
    assert_storage_tier!(
        &fixture.ctx.env,
        &fixture.contract_id,
        added.key.as_str(),
        StorageTier::Persistent
    );
    assert!(added.live_until.is_some(), "persistent entries carry a TTL");
}

#[test]
fn billing_rewrites_the_schedule_without_adding_entries() {
    let mut fixture = Fixture::new();
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let before = snapshot(&fixture);
    fixture.bill();
    let after = snapshot(&fixture);

    let diff = before.diff(&after);
    assert!(diff.added().is_empty() && diff.removed().is_empty());
    assert_eq!(diff.modified().len(), 1);
    assert_eq!(diff.modified()[0].tier, StorageTier::Persistent);
}

#[test]
fn cancellation_rewrites_the_same_entry() {
    let fixture = Fixture::new();
    fixture.subscribe();

    let before = snapshot(&fixture);
    fixture.cancel();
    let after = snapshot(&fixture);

    let diff = before.diff(&after);
    assert!(diff.added().is_empty() && diff.removed().is_empty());
    assert_eq!(diff.modified().len(), 1);
    assert_eq!(after.len(), before.len());
}

#[test]
fn a_rejected_payment_leaves_storage_unchanged() {
    let mut fixture = Fixture::with_limits(FEE, INTERVAL, 10_000, 0, None);
    fixture.subscribe();
    fixture.advance_time(INTERVAL);

    let before = snapshot(&fixture);
    assert_eq!(
        fixture.try_bill().unwrap_err(),
        soroban_testkit_example_subscription::SubscriptionError::PaymentFailed
    );
    before.assert_unchanged(&snapshot(&fixture));
}

#[test]
fn a_long_running_subscription_still_holds_one_entry() {
    let mut fixture = Fixture::new();
    fixture.subscribe();

    for _ in 0..3 {
        fixture.advance_time(INTERVAL);
        fixture.bill();
    }

    assert_storage_tier_count!(fixture.ctx.env, fixture.contract_id, persistent = 1);
    assert_eq!(snapshot(&fixture).len(), 1);
}

#[test]
fn the_subscription_key_is_namespaced() {
    let mut auditor = NamespaceAuditor::default();
    auditor
        .record("subscription", "DataKey::Subscription")
        .record("subscription", "DataKey::Subscription(Address)");

    let report = auditor.audit();
    assert!(!report.has_collisions());
    assert!(report.un_namespaced_keys().is_empty());
    assert!(report.is_clean());
    assert_eq!(
        report.modules().get("subscription").unwrap().len(),
        2,
        "the auditor keeps both recorded spellings under one module"
    );
}

#[test]
fn reading_the_schedule_does_not_touch_storage() {
    let fixture = Fixture::new();
    fixture.subscribe();
    let before = snapshot(&fixture);

    fixture.client().subscription(&fixture.subscriber);
    fixture.client().is_active(&fixture.subscriber);
    fixture.client().is_billing_due(&fixture.subscriber);

    before.assert_unchanged(&snapshot(&fixture));
}

#[test]
fn a_fresh_contract_has_no_storage_to_audit() {
    let fixture = Fixture::new();
    assert_storage_tier_count!(fixture.ctx.env, fixture.contract_id, {
        instance: 0,
        persistent: 0,
        temporary: 0,
    });

    // The next billing boundary is still readable as a fact about the clock even
    // when no agreement exists.
    assert_eq!(fixture.next_billing, NEXT_BILLING);
}
