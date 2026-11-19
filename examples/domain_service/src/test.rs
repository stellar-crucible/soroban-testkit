extern crate std;

// The fixture layer on its own: what `TestContext` hands a registry test, how
// the builder shapes it, and what advancing the clock does to a term. The other
// three files in this package each take one of the remaining crates.
use soroban_sdk::Address;
use soroban_sdk::String;
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

use crate::{DomainService, DomainServiceClient, SECONDS_PER_YEAR};

/// A context with `users` holders and a mocked-auth policy, which is what a
/// registry test usually wants: the contract's `require_auth` calls succeed
/// without the test describing them.
fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

/// Registers the registry in `ctx`, initializes it with the fixture's admin as
/// both operator and treasury, and gives the fixture's users credits to spend.
///
/// Registration belongs to the env, so this is the step a test repeats after
/// `TestContext::reset`.
fn registry(ctx: &TestContext) -> (Address, DomainServiceClient<'_>) {
    let contract_id = ctx.env.register(DomainService, ());
    let client = DomainServiceClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin, &ctx.admin, &100, &604_800);
    for user in ctx.users.iter() {
        client.deposit(user, &1_000);
    }
    (contract_id, client)
}

/// A `soroban_sdk::String` owned by `ctx`, which is the name type the contract
/// takes.
fn name(ctx: &TestContext, text: &str) -> String {
    String::from_str(&ctx.env, text)
}

#[test]
fn initialize_records_the_operator_the_treasury_and_the_terms() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    assert_eq!(client.admin(), ctx.admin);
    assert_eq!(client.treasury(), ctx.admin);
    assert_eq!(client.base_fee(), 100);
    assert_eq!(client.grace_period(), 604_800);
}

#[test]
#[should_panic(expected = "registry already initialized")]
fn initialize_twice_points_at_the_call_that_already_happened() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.initialize(&ctx.admin, &ctx.admin, &100, &604_800);
}

#[test]
fn registering_a_name_records_the_owner_and_the_end_of_the_term() {
    let mut ctx = context(1);
    ctx.set_timestamp(1_700_000_000);
    let (_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let label = name(&ctx, "alice");

    let domain = client.register(&owner, &label, &2);

    // The term is counted from the ledger clock, not from wall time, so a test
    // can state the expiry as a number it chose.
    assert_eq!(domain.owner, owner);
    assert_eq!(domain.expires_at, 1_700_000_000 + 2 * SECONDS_PER_YEAR);
    assert_eq!(client.owner_of(&label), Some(owner));
    assert_eq!(client.expires_at(&label), Some(domain.expires_at));
}

#[test]
fn advancing_the_clock_is_what_expires_a_name() {
    let mut ctx = context(1);
    ctx.set_timestamp(1_700_000_000);
    // The client is built on a clone of the env, so advancing the fixture's
    // clock below does not fight the borrow the registry helper returned.
    let env = ctx.env.clone();
    let (id, _) = registry(&ctx);
    let client = DomainServiceClient::new(&env, &id);
    let label = name(&ctx, "alice");
    client.register(ctx.user(0), &label, &1);

    // A day short of the term, the registry still holds it.
    ctx.advance_time(SECONDS_PER_YEAR - 86_400);
    assert!(!client.is_available(&label));
    assert_eq!(client.owner_of(&label), Some(ctx.user(0).clone()));

    // Past the term and past the grace period, the name is free again.
    ctx.advance_time(86_400 + 604_800 + 1);
    assert!(client.is_available(&label));
}

#[test]
fn a_name_inside_its_grace_period_is_still_unavailable() {
    let mut ctx = context(1);
    let env = ctx.env.clone();
    let (id, _) = registry(&ctx);
    let client = DomainServiceClient::new(&env, &id);
    let label = name(&ctx, "alice");
    client.register(ctx.user(0), &label, &1);

    // Expired yesterday, but the previous owner has a week to renew.
    ctx.advance_time(SECONDS_PER_YEAR + 1);
    assert!(!client.is_available(&label));
}

#[test]
fn reset_keeps_the_holders_and_empties_the_registry() {
    let mut ctx = context(1);
    let (_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let label = name(&ctx, "alice");
    client.register(&owner, &label, &1);
    assert!(client.owner_of(&label).is_some());

    // Same identities, brand-new ledger: the holders survive, the storage does
    // not, so the registry has to be registered and initialized again.
    ctx.reset();
    let (_id, client) = registry(&ctx);

    // The fixture carries the identities across the reset, but every object
    // handle belongs to the env that minted it: the name is rebuilt in the new
    // env before it is asked about.
    let label = name(&ctx, "alice");
    assert_eq!(client.owner_of(&label), None);
    assert!(client.is_available(&label));
    assert_eq!(client.balance_of(ctx.user(0)), 1_000);
}

#[test]
fn add_user_grows_the_context_for_the_next_owner() {
    let mut ctx = context(1);
    let late = ctx.add_user().clone();
    let (_id, client) = registry(&ctx);

    let domain = client.register(&late, &name(&ctx, "bob"), &1);

    assert_eq!(ctx.users.len(), 2);
    assert_eq!(domain.owner, late);
}

#[test]
#[should_panic(expected = "user index out of bounds: index is 2 but context holds 2 users")]
fn asking_for_a_holder_that_was_never_built_points_at_with_users() {
    let ctx = context(2);

    ctx.user(2);
}

#[test]
#[should_panic(expected = "domain name must be 3 to 32 characters")]
fn a_name_shorter_than_the_namespace_allows_is_refused() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.register(ctx.user(0), &name(&ctx, "ab"), &1);
}

#[test]
fn is_valid_name_answers_without_registering_anything() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    assert!(client.is_valid_name(&name(&ctx, "alice")));
    assert!(client.is_valid_name(&name(&ctx, "a-b9")));
    assert!(!client.is_valid_name(&name(&ctx, "Alice")));
    assert!(!client.is_valid_name(&name(&ctx, "-alice")));
    assert!(!client.is_valid_name(&name(&ctx, "alice-")));
    assert!(!client.is_valid_name(&name(&ctx, "al ice")));

    // Nothing above registered a name, so the registry is still empty.
    assert_eq!(client.domain(&name(&ctx, "alice")), None);
}
