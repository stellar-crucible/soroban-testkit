extern crate std;

// This file is the fixture layer on its own: what `TestContext` hands a test,
// how the builder shapes it, and what the ledger helpers do to it. The other
// three files in this package each take one of the remaining crates.
use soroban_sdk::Address;
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

use crate::{TokenContract, TokenContractClient};

/// A context with `users` holders and a mocked-auth policy, which is what a
/// token test usually wants: the contract's `require_auth` calls succeed without
/// the test describing them.
fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

/// Registers the token in `ctx` and initializes it with the context's admin.
///
/// Registration belongs to the env, so this is the step a test repeats after
/// `TestContext::reset`.
fn token(ctx: &TestContext) -> (Address, TokenContractClient<'_>) {
    let contract_id = ctx.env.register(TokenContract, ());
    let client = TokenContractClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin);
    (contract_id, client)
}

#[test]
fn initialize_records_the_admin_and_starts_the_supply_at_zero() {
    let ctx = context(1);
    let (_id, client) = token(&ctx);

    // The fixture's admin is an ordinary address the contract can adopt, and the
    // client reads back exactly what was stored.
    assert_eq!(client.admin(), ctx.admin.clone());
    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.balance_of(ctx.user(0)), 0);
}

#[test]
fn the_builder_hands_out_as_many_distinct_holders_as_the_test_asks_for() {
    let ctx = context(3);

    assert_eq!(ctx.users.len(), 3);
    // `user(index)` is the checked accessor: it answers for the same address
    // indexing does, and it is the one that explains itself when the index is
    // beyond what was built.
    assert_eq!(ctx.user(2), &ctx.users[2]);
    assert_ne!(ctx.user(0), ctx.user(1));
}

#[test]
#[should_panic(expected = "user index out of bounds: index is 2 but context holds 2 users")]
fn asking_for_a_holder_that_was_never_built_points_at_with_users() {
    let ctx = context(2);

    ctx.user(2);
}

#[test]
fn the_fixture_starts_at_a_known_ledger_and_the_clock_helpers_move_it() {
    let mut ctx = context(1);

    assert_eq!(ctx.timestamp(), 0);
    assert_eq!(ctx.sequence(), 0);

    assert_eq!(ctx.advance_time(60), 60);
    assert_eq!(ctx.advance_ledger(2), 2);
    assert_eq!(ctx.timestamp(), 60);
    assert_eq!(ctx.sequence(), 2);

    ctx.set_timestamp(1_000);
    assert_eq!(ctx.timestamp(), 1_000);
}

#[test]
fn minting_through_the_client_moves_supply_and_balance_together() {
    let ctx = context(2);
    let (_id, client) = token(&ctx);
    let holder = ctx.user(0).clone();

    client.mint(&holder, &1_000);

    assert_eq!(client.balance_of(&holder), 1_000);
    assert_eq!(client.total_supply(), 1_000);
    // A holder the contract has never paid still reads zero rather than failing.
    assert_eq!(client.balance_of(ctx.user(1)), 0);
}

#[test]
fn reset_keeps_the_holders_and_empties_the_chain() {
    let mut ctx = context(1);
    let (_id, client) = token(&ctx);
    let holder = ctx.user(0).clone();
    client.mint(&holder, &500);
    assert_eq!(client.balance_of(&holder), 500);

    // Same identities, brand-new ledger: the holders survive, the storage does
    // not, so the token has to be registered again.
    ctx.reset();
    let (_id, client) = token(&ctx);

    assert_eq!(client.balance_of(&holder), 0);
    assert_eq!(client.total_supply(), 0);
}

#[test]
fn add_user_grows_the_context_for_the_next_holder() {
    let mut ctx = context(1);
    let late = ctx.add_user().clone();
    let (_id, client) = token(&ctx);

    client.mint(&late, &7);

    assert_eq!(ctx.users.len(), 2);
    assert_eq!(client.balance_of(&late), 7);
}
