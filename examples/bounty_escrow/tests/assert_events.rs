// One crate per file, which is the point of splitting this suite up: this file
// is `soroban-testkit-assert`. It asserts on the events a bounty publishes and
// on the authorizations each transition requires — nothing about budgets,
// storage or generated inputs.
extern crate std;

use std::vec::Vec as StdVec;

use soroban_sdk::{testutils::Ledger as _, token::StellarAssetClient, Address, Vec};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_bounty_escrow::{
    BountyEscrowContract, BountyEscrowContractClient, BountyStatus,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

/// Registers the escrow and a funded Stellar asset contract.
///
/// A bounty needs three kinds of identity — creator, contributor, reviewer — and
/// the token's admin besides, so the fixture asks for five users and spends
/// the extra one on a reviewer the tests never use.
fn deploy(ctx: &TestContext) -> (Address, Address, BountyEscrowContractClient<'_>) {
    let contract_id = ctx.env.register(BountyEscrowContract, ());
    let client = BountyEscrowContractClient::new(&ctx.env, &contract_id);
    let token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    StellarAssetClient::new(&ctx.env, &token).mint(ctx.user(0), &10_000);
    (contract_id, token, client)
}

fn reviewer_list(ctx: &TestContext) -> Vec<Address> {
    let mut reviewers = Vec::new(&ctx.env);
    reviewers.push_back(ctx.user(2).clone());
    reviewers.push_back(ctx.user(3).clone());
    reviewers
}

/// Opens a one-hundred-token bounty with a two-of-two reviewer threshold.
fn open(client: &BountyEscrowContractClient<'_>, ctx: &TestContext, token: &Address) -> u64 {
    client.create(
        ctx.user(0),
        token,
        &100,
        &reviewer_list(ctx),
        &2,
        &(ctx.timestamp() + 86_400),
    )
}

/// Opens, submits and returns the bounty id.
fn submitted(client: &BountyEscrowContractClient<'_>, ctx: &TestContext, token: &Address) -> u64 {
    let id = open(client, ctx, token);
    client.submit(&id, ctx.user(1));
    id
}

#[test]
fn a_created_event_carries_the_bounty_terms() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (contract_id, token, client) = deploy(&ctx);
    let deadline = ctx.timestamp() + 86_400;

    let id = client.create(
        ctx.user(0),
        &token,
        &100,
        &reviewer_list(&ctx),
        &2,
        &deadline,
    );

    // The terms are what a client reads to know what it is funding, so the
    // event repeats all of them rather than only the id.
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("created")
        .assert_data_matches(|data| {
            data.field("id")
                .and_then(|value| value.deserialize::<u64>())
                == Some(id)
                && data
                    .field("creator")
                    .and_then(|value| value.deserialize::<Address>())
                    == Some(ctx.users[0].clone())
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(100)
                && data
                    .field("reviewers")
                    .and_then(|value| value.deserialize::<u32>())
                    == Some(2)
                && data
                    .field("threshold")
                    .and_then(|value| value.deserialize::<u32>())
                    == Some(2)
                && data
                    .field("deadline")
                    .and_then(|value| value.deserialize::<u64>())
                    == Some(deadline)
        });
}

#[test]
fn the_created_payload_names_every_field_it_carries() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    open(&client, &ctx, &token);

    EventMatcher::new(&ctx.env)
        .with_topic("created")
        .assert_data_matches(|data| {
            let mut names = data.fields();
            names.sort();
            names
                == [
                    "amount",
                    "creator",
                    "deadline",
                    "id",
                    "reviewers",
                    "threshold",
                ]
        });
}

#[test]
fn a_submitted_event_names_the_contributor() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = open(&client, &ctx, &token);

    client.submit(&id, ctx.user(1));

    EventMatcher::new(&ctx.env)
        .with_topic("submitted")
        .assert_data_matches(|data| {
            data.field("id")
                .and_then(|value| value.deserialize::<u64>())
                == Some(id)
                && data
                    .field("contributor")
                    .and_then(|value| value.deserialize::<Address>())
                    == Some(ctx.users[1].clone())
        });
}

#[test]
fn every_approval_reports_the_running_count() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    client.approve(&id, ctx.user(2));

    EventMatcher::new(&ctx.env)
        .with_topic("approved")
        .assert_data_matches(|data| {
            data.field("approvals")
                .and_then(|value| value.deserialize::<u32>())
                == Some(1)
                && data
                    .field("threshold")
                    .and_then(|value| value.deserialize::<u32>())
                    == Some(2)
        });

    client.approve(&id, ctx.user(3));

    // The second signature is the one that reached the threshold, so its event
    // reports 2 — the count belongs to the call that moved it.
    EventMatcher::new(&ctx.env)
        .with_topic("approved")
        .assert_data_matches(|data| {
            data.field("approvals")
                .and_then(|value| value.deserialize::<u32>())
                == Some(2)
        });
}

#[test]
fn a_release_event_fires_when_the_threshold_is_reached() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (contract_id, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    let mut log = EventLog::new(&ctx.env);
    client.approve(&id, ctx.user(2));
    log.collect();
    client.approve(&id, ctx.user(3));
    log.collect();

    // One release, after two approvals: the log is what makes this a statement
    // about the sequence rather than about the last call.
    log.matcher().with_topic("approved").assert_count(2);
    log.matcher().with_topic("released").assert_count(1);

    // The payout goes through the token contract, which publishes its own
    // transfer event in the same invocation, so the log carries the bounty's
    // three events alongside it. Filtering on the first topic is how a sequence
    // assertion reads only the bounty's side of the call.
    assert!(log.len() >= 3, "the log holds {} events", log.len());
    let bounty_events: StdVec<String> = log
        .topics()
        .iter()
        .filter(|topics| topics.first().map(String::as_str) == Some("bounty"))
        .map(|topics| topics[1].clone())
        .collect();
    assert_eq!(bounty_events, ["approved", "approved", "released"]);

    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("released")
        .assert_data_matches(|data| {
            data.field("contributor")
                .and_then(|value| value.deserialize::<Address>())
                == Some(ctx.users[1].clone())
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(100)
        });
}

#[test]
fn no_release_before_the_threshold_is_reached() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (contract_id, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    let mut log = EventLog::new(&ctx.env);
    client.approve(&id, ctx.user(2));
    log.collect();

    log.matcher().with_topic("released").assert_not_emitted();
    assert_eq!(client.get(&id).status, BountyStatus::Submitted);

    // One signature of two, so the escrow is untouched.
    let token_client = soroban_sdk::token::Client::new(&ctx.env, &token);
    assert_eq!(token_client.balance(&contract_id), 100);
    assert_eq!(token_client.balance(ctx.user(1)), 0);
}

#[test]
fn a_refund_event_carries_the_amount() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);
    // Move the ledger through the env the client borrows rather than through
    // `TestContext::advance_time`, which would borrow the whole fixture mutably.
    ctx.env.ledger().set_timestamp(ctx.timestamp() + 86_401);

    client.refund(&id);

    EventMatcher::new(&ctx.env)
        .with_topic("refunded")
        .assert_data_matches(|data| {
            data.field("id")
                .and_then(|value| value.deserialize::<u64>())
                == Some(id)
                && data
                    .field("creator")
                    .and_then(|value| value.deserialize::<Address>())
                    == Some(ctx.users[0].clone())
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(100)
        });
    // A refund is not a release: the two events never share a call.
    EventMatcher::new(&ctx.env)
        .with_topic("released")
        .assert_not_emitted();
}

#[test]
fn creating_a_bounty_records_the_creators_authorization() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let deadline = ctx.timestamp() + 86_400;

    client.create(
        ctx.user(0),
        &token,
        &100,
        &reviewer_list(&ctx),
        &2,
        &deadline,
    );

    // Under the builder's default policy the requirement is recorded rather
    // than enforced, so the matcher can read back who authorized what.
    assert_auth_matches!(ctx.env, {
        invoker: ctx.users[0],
        function: "create",
        args: [_, _, 100i128, _, 2u32, deadline],
    });
}

#[test]
fn submitting_records_the_contributors_authorization() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = open(&client, &ctx, &token);

    client.submit(&id, ctx.user(1));

    assert_auth_matches!(ctx.env, {
        invoker: ctx.users[1],
        function: "submit",
        args: [id, _],
    });
}

#[test]
fn approving_records_the_reviewers_authorization() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    client.approve(&id, ctx.user(2));

    assert_auth_matches!(ctx.env, {
        invoker: ctx.users[2],
        function: "approve",
        args: [id, _],
    });
}

#[test]
fn a_refund_is_authorized_by_the_creator() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);
    ctx.env.ledger().set_timestamp(ctx.timestamp() + 86_401);

    client.refund(&id);

    assert_auth_matches!(ctx.env, {
        invoker: ctx.users[0],
        function: "refund",
        args: [id],
    });
}

#[test]
fn the_matcher_counts_one_signature_and_none_for_a_read() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    client.approve(&id, ctx.user(2));
    AuthMatcher::new(&ctx.env).assert_auth_count(1);

    // A read requires nothing, so the same environment now records nothing:
    // the count is per invocation, not per test.
    client.get(&id);
    AuthMatcher::new(&ctx.env).assert_no_auth_required();
}

#[test]
fn an_address_that_is_not_a_reviewer_cannot_approve() {
    let ctx = TestContextBuilder::new().with_users(6).build();
    let (contract_id, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    // `user(5)` was built by the fixture but named by no bounty.
    assert!(client.try_approve(&id, ctx.user(5)).is_err());

    assert_eq!(client.approval_count(&id), 0);
    assert_eq!(
        soroban_sdk::token::Client::new(&ctx.env, &token).balance(&contract_id),
        100
    );
}

#[test]
fn a_reviewer_cannot_approve_twice() {
    let ctx = TestContextBuilder::new().with_users(5).build();
    let (_, token, client) = deploy(&ctx);
    let id = submitted(&client, &ctx, &token);

    client.approve(&id, ctx.user(2));

    // One key signing twice would let a two-of-two bounty be released without a
    // second party, so the duplicate is refused rather than counted.
    assert!(client.try_approve(&id, ctx.user(2)).is_err());
    assert_eq!(client.approval_count(&id), 1);
    assert_eq!(client.get(&id).status, BountyStatus::Submitted);
}
