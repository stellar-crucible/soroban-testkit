extern crate std;

// This file is the fixture layer on its own: the identities `TestContext`
// hands a test, how the builder shapes them, and what the ledger helpers do to
// a deadline. The other three files in this package each take one of the
// remaining crates.
use soroban_sdk::{token::StellarAssetClient, Address, Vec};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

use crate::{BountyEscrowContract, BountyEscrowContractClient, BountyStatus};

/// A context with `users` holders. Index 0 is the creator, index 1 the
/// contributor, and every index from 2 is a reviewer.
fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

/// Registers the escrow and a Stellar asset contract funded with `funding` for
/// the context's creator.
///
/// Registration belongs to the env, so this is the step a test repeats after
/// `TestContext::reset`.
fn deploy(ctx: &TestContext, funding: i128) -> (Address, Address, BountyEscrowContractClient<'_>) {
    let contract_id = ctx.env.register(BountyEscrowContract, ());
    let client = BountyEscrowContractClient::new(&ctx.env, &contract_id);
    let token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    StellarAssetClient::new(&ctx.env, &token).mint(ctx.user(0), &funding);
    (contract_id, token, client)
}

/// The first `count` reviewer identities of a context built for them.
fn reviewers(ctx: &TestContext, count: usize) -> Vec<Address> {
    let mut list = Vec::new(&ctx.env);
    for index in 0..count {
        list.push_back(ctx.user(2 + index).clone());
    }
    list
}

/// Opens a bounty funded from the creator's balance and returns its id.
fn open(
    client: &BountyEscrowContractClient<'_>,
    ctx: &TestContext,
    token: &Address,
    amount: i128,
) -> u64 {
    let deadline = ctx.timestamp() + 86_400;
    client.create(
        ctx.user(0),
        token,
        &amount,
        &reviewers(ctx, 3),
        &2,
        &deadline,
    )
}

#[test]
fn creating_a_bounty_locks_the_creators_funds_in_the_contract() {
    let ctx = context(5);
    let (contract_id, token, client) = deploy(&ctx, 5_000);
    let amount = 1_200;

    let id = open(&client, &ctx, &token, amount);

    let token_client = soroban_sdk::token::Client::new(&ctx.env, &token);
    assert_eq!(token_client.balance(&ctx.users[0]), 3_800);
    assert_eq!(token_client.balance(&contract_id), amount);

    let bounty = client.get(&id);
    assert_eq!(bounty.creator, ctx.users[0]);
    assert_eq!(bounty.token, token);
    assert_eq!(bounty.amount, amount);
    assert_eq!(bounty.contributor, None);
    assert_eq!(bounty.reviewers.len(), 3);
    assert_eq!(bounty.approvals.len(), 0);
    assert_eq!(bounty.threshold, 2);
    assert_eq!(bounty.status, BountyStatus::Open);
}

#[test]
fn the_builder_hands_out_creator_contributor_and_reviewers() {
    let ctx = context(5);

    assert_eq!(ctx.users.len(), 5);
    assert_eq!(ctx.user(3), &ctx.users[3]);
    assert_ne!(ctx.user(0), ctx.user(1));
    assert_ne!(ctx.user(2), ctx.user(4));
}

#[test]
#[should_panic(expected = "user index out of bounds: index is 3 but context holds 3 users")]
fn asking_for_a_reviewer_that_was_never_built_points_at_with_users() {
    let ctx = context(3);

    ctx.user(3);
}

#[test]
fn the_fixture_clock_is_what_the_deadline_is_measured_against() {
    let mut ctx = context(3);
    ctx.set_timestamp(1_700_000_000);
    ctx.advance_time(60);
    let (_, token, client) = deploy(&ctx, 100);
    let deadline = ctx.timestamp() + 3_600;

    let id = client.create(
        ctx.user(0),
        &token,
        &100,
        &reviewers(&ctx, 1),
        &1,
        &deadline,
    );

    // The contract stores the absolute unix timestamp the fixture clock
    // reported, not an offset: a test that moves the clock again is moving
    // against the same number the contract will compare.
    assert_eq!(client.get(&id).deadline, 1_700_003_660);
}

#[test]
fn a_bounty_cannot_be_created_with_an_empty_reviewer_set() {
    let ctx = context(3);
    let (_, token, client) = deploy(&ctx, 1_000);
    let empty = Vec::new(&ctx.env);

    assert!(client
        .try_create(ctx.user(0), &token, &100, &empty, &1, &86_400)
        .is_err());
}

#[test]
fn a_threshold_above_the_reviewer_count_is_refused() {
    let ctx = context(5);
    let (_, token, client) = deploy(&ctx, 1_000);

    assert!(client
        .try_create(ctx.user(0), &token, &100, &reviewers(&ctx, 2), &3, &86_400)
        .is_err());
}

#[test]
fn a_threshold_of_zero_is_refused() {
    let ctx = context(3);
    let (_, token, client) = deploy(&ctx, 1_000);

    assert!(client
        .try_create(ctx.user(0), &token, &100, &reviewers(&ctx, 1), &0, &86_400)
        .is_err());
}

#[test]
fn duplicate_reviewers_are_refused() {
    let ctx = context(4);
    let (_, token, client) = deploy(&ctx, 1_000);
    let mut duplicated = Vec::new(&ctx.env);
    duplicated.push_back(ctx.user(2).clone());
    duplicated.push_back(ctx.user(2).clone());

    // Two entries for one address would let one key satisfy a two-of-two
    // bounty, so the contract refuses the configuration outright.
    assert!(client
        .try_create(ctx.user(0), &token, &100, &duplicated, &2, &86_400)
        .is_err());
}

#[test]
fn a_deadline_in_the_past_is_refused() {
    let ctx = context(3);
    let (_, token, client) = deploy(&ctx, 1_000);

    let created = client.try_create(ctx.user(0), &token, &100, &reviewers(&ctx, 1), &1, &0);

    // The fixture opens at timestamp 0, so a deadline of 0 is not "now"; it is
    // already behind the ledger.
    assert!(created.is_err());
}

#[test]
fn submitting_records_the_contributor_and_moves_the_status() {
    let ctx = context(5);
    let (_, token, client) = deploy(&ctx, 1_000);
    let id = open(&client, &ctx, &token, 500);

    client.submit(&id, ctx.user(1));

    let bounty = client.get(&id);
    assert_eq!(bounty.contributor, Some(ctx.user(1).clone()));
    assert_eq!(bounty.status, BountyStatus::Submitted);
    assert_eq!(client.approval_count(&id), 0);
    // Submitting moves no money: the escrow is still the whole amount.
    assert_eq!(
        soroban_sdk::token::Client::new(&ctx.env, &token).balance(&client.address),
        500
    );
}

#[test]
fn a_second_bounty_gets_the_next_id_and_holds_its_own_escrow() {
    let ctx = context(5);
    let (_, token, client) = deploy(&ctx, 5_000);

    let first = open(&client, &ctx, &token, 700);
    let second = open(&client, &ctx, &token, 900);

    assert_eq!((first, second), (1, 2));
    assert_eq!(client.next_bounty_id(), 3);
    assert_eq!(client.get(&first).amount, 700);
    assert_eq!(client.get(&second).amount, 900);

    let token_client = soroban_sdk::token::Client::new(&ctx.env, &token);
    assert_eq!(token_client.balance(&client.address), 1_600);
    assert_eq!(token_client.balance(&ctx.users[0]), 3_400);
}

#[test]
fn reset_keeps_the_identities_and_empties_the_chain() {
    let mut ctx = context(5);
    let (_, token, client) = deploy(&ctx, 1_000);
    let creator = ctx.user(0).clone();
    let first = open(&client, &ctx, &token, 400);
    assert_eq!(first, 1);

    // Same identities, brand-new ledger: the token and the escrow are gone,
    // because registration belongs to the env that created it.
    ctx.reset();
    let (contract_id, token, client) = deploy(&ctx, 1_000);

    assert_eq!(ctx.user(0), &creator);
    assert_eq!(client.next_bounty_id(), 1);
    let id = open(&client, &ctx, &token, 400);
    assert_eq!(id, 1);
    assert_eq!(
        soroban_sdk::token::Client::new(&ctx.env, &token).balance(&contract_id),
        400
    );
}

#[test]
fn add_user_grows_the_context_for_a_late_reviewer() {
    let mut ctx = context(3);
    let late = ctx.add_user().clone();
    let (_, token, client) = deploy(&ctx, 1_000);
    let mut list = Vec::new(&ctx.env);
    list.push_back(late.clone());

    let id = client.create(ctx.user(0), &token, &300, &list, &1, &86_400);

    assert_eq!(ctx.users.len(), 4);
    assert_eq!(client.reviewers(&id), list);
    assert_eq!(client.get(&id).reviewers.len(), 1);
}
