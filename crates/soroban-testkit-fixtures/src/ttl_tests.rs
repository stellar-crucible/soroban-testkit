//! TTL fixtures across all three storage tiers under sequence advancement.

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env, Symbol};

use crate::{TestContext, TtlInspector, TtlStatus};
use soroban_testkit_core::StorageTier;

const KEY: Symbol = symbol_short!("key");

#[contract]
pub struct Rent;

#[contractimpl]
impl Rent {
    pub fn put_instance(env: Env, value: u32) {
        env.storage().instance().set(&KEY, &value);
    }

    pub fn put_persistent(env: Env, value: u32) {
        env.storage().persistent().set(&KEY, &value);
    }

    pub fn put_temporary(env: Env, value: u32) {
        env.storage().temporary().set(&KEY, &value);
    }

    pub fn get_temporary(env: Env) -> Option<u32> {
        env.storage().temporary().get(&KEY)
    }

    pub fn has_temporary(env: Env) -> bool {
        env.storage().temporary().has(&KEY)
    }

    pub fn extend_instance(env: Env, threshold: u32, extend_to: u32) {
        env.storage().instance().extend_ttl(threshold, extend_to);
    }

    pub fn extend_persistent(env: Env, threshold: u32, extend_to: u32) {
        env.storage()
            .persistent()
            .extend_ttl(&KEY, threshold, extend_to);
    }

    pub fn extend_temporary(env: Env, threshold: u32, extend_to: u32) {
        env.storage()
            .temporary()
            .extend_ttl(&KEY, threshold, extend_to);
    }
}

fn setup() -> (TestContext, Address, RentClient<'static>) {
    let ctx = TestContext::new();
    let id = ctx.env.register(Rent, ());
    let client = RentClient::new(&ctx.env, &id);
    (ctx, id, client)
}

#[test]
fn issue_example_temporary_entry_expires_and_reads_empty() {
    let (ctx, contract_id, client) = setup();
    client.put_temporary(&1);

    let ttl = TtlInspector::for_key(&ctx.env, &contract_id, &KEY);
    assert!(ttl.live_until_ledger > ctx.ledger_sequence());

    let at = ctx.fast_forward_to_expiration(&contract_id, &KEY);

    assert_eq!(at, ttl.live_until_ledger + 1);
    assert_eq!(ctx.ledger_sequence(), at);
    ctx.assert_temporary_expired(&contract_id, &KEY);
    assert_eq!(client.get_temporary(), None);
    assert!(!client.has_temporary());
}

#[test]
fn temporary_entry_is_readable_on_its_last_live_ledger() {
    let (ctx, id, client) = setup();
    client.put_temporary(&4);

    let last = ctx.fast_forward_to_last_live_ledger(&id, &KEY);

    let ttl = ctx.ttl(&id, &KEY);
    assert_eq!(ttl.current_ledger, last);
    assert_eq!(ttl.ttl(), 0);
    assert_eq!(ttl.status(), TtlStatus::Live);
    assert_eq!(client.get_temporary(), Some(4));
}

#[test]
fn temporary_entry_ttl_counts_down_as_ledgers_advance() {
    let (mut ctx, id, client) = setup();
    client.put_temporary(&1);
    let start = ctx.ttl(&id, &KEY);

    ctx.advance_ledger(5);
    let later = ctx.ttl(&id, &KEY);

    assert_eq!(later.live_until_ledger, start.live_until_ledger);
    assert_eq!(later.ttl(), start.ttl() - 5);
    assert_eq!(
        later.ledgers_until_expiration(),
        start.ledgers_until_expiration() - 5
    );
}

#[test]
#[should_panic(expected = "to have expired at ledger")]
fn assert_temporary_expired_fails_while_the_entry_is_live() {
    let (ctx, id, client) = setup();
    client.put_temporary(&1);

    ctx.assert_temporary_expired(&id, &KEY);
}

#[test]
fn persistent_entry_transitions_to_archived_past_its_ttl() {
    let (ctx, id, client) = setup();
    client.put_persistent(&1);
    let ttl = ctx.ttl(&id, &KEY);
    assert_eq!(ttl.tier, StorageTier::Persistent);
    assert_eq!(ttl.min_ttl(), ttl.thresholds.min_persistent_entry_ttl);

    ctx.fast_forward_to_last_live_ledger(&id, &KEY);
    assert_eq!(ctx.ttl(&id, &KEY).status(), TtlStatus::Live);

    ctx.fast_forward_to_expiration(&id, &KEY);
    ctx.assert_persistent_archived(&id, &KEY);
}

#[test]
fn an_archived_persistent_entry_is_swept_by_archive_expired() {
    let (mut ctx, id, client) = setup();
    client.put_persistent(&1);
    ctx.fast_forward_to_expiration(&id, &KEY);

    let simulator = ctx.archive_expired(0);

    assert_eq!(simulator.len(), 1);
    assert!(TtlInspector::try_in_tier(&ctx.env, &id, &KEY, StorageTier::Persistent).is_none());
}

#[test]
#[should_panic(expected = "to be archived at ledger")]
fn assert_persistent_archived_fails_while_the_entry_is_live() {
    let (ctx, id, client) = setup();
    client.put_persistent(&1);

    ctx.assert_persistent_archived(&id, &KEY);
}

#[test]
fn instance_key_expires_with_the_instance() {
    let (ctx, id, client) = setup();
    client.put_instance(&1);
    let ttl = ctx.ttl(&id, &KEY);
    assert_eq!(ttl.tier, StorageTier::Instance);
    assert_eq!(
        ttl.live_until_ledger,
        TtlInspector::instance(&ctx.env, &id).live_until_ledger
    );

    ctx.fast_forward_to_expiration(&id, &KEY);

    assert_eq!(ctx.ttl(&id, &KEY).status(), TtlStatus::Archived);
}

#[test]
fn extend_ttl_on_each_tier_is_verified_against_the_host_rules() {
    let (mut ctx, id, client) = setup();
    client.put_instance(&1);
    ctx.advance_ledger(10);

    let instance =
        ctx.observe_ttl_extension(&id, &KEY, || client.extend_instance(&50_000, &50_000));
    instance.assert_matches_extend_ttl(50_000, 50_000);
    instance.assert_extended_to(50_000);

    client.put_persistent(&1);
    let persistent = ctx.observe_ttl_extension(&id, &symbol_short!("key"), || {
        client.extend_persistent(&10_000, &20_000)
    });
    persistent.assert_matches_extend_ttl(10_000, 20_000);

    client.put_temporary(&1);
    let before = TtlInspector::temporary(&ctx.env, &id, &KEY);
    ctx.advance_ledger(before.ttl() / 2);
    let temporary =
        crate::TtlExtension::observe_from(&ctx.env, before.refresh(&ctx.env).unwrap(), || {
            client.extend_temporary(&100, &1_000)
        });
    temporary.assert_extended_to(1_000);
    temporary.assert_matches_extend_ttl(100, 1_000);
}

#[test]
fn extend_ttl_keeps_a_temporary_entry_alive_past_its_original_expiry() {
    let (ctx, id, client) = setup();
    client.put_temporary(&9);
    let original = ctx.ttl(&id, &KEY);

    let extension =
        ctx.observe_ttl_extension(&id, &KEY, || client.extend_temporary(&1_000, &1_000));
    assert!(extension.was_extended());

    ctx.env
        .ledger()
        .set_sequence_number(original.expiration_ledger());
    assert_eq!(
        client.get_temporary(),
        Some(9),
        "the extension carried it past the old boundary"
    );

    ctx.fast_forward_to_expiration(&id, &KEY);
    ctx.assert_temporary_expired(&id, &KEY);
}

#[test]
fn extend_ttl_below_the_threshold_does_not_move_the_entry() {
    let (ctx, id, client) = setup();
    client.put_persistent(&1);
    let ttl = ctx.ttl(&id, &KEY);

    let extension = ctx.observe_ttl_extension(&id, &KEY, || {
        client.extend_persistent(&(ttl.ttl() - 1), &(ttl.ttl() + 100))
    });

    extension.assert_not_extended();
}
