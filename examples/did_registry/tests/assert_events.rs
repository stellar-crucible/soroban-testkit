// One crate per file, which is the point of splitting this suite up: this file
// is `soroban-testkit-assert`. It asserts on the events the registry publishes
// and on the authorizations it requires — nothing about budgets or generated
// inputs.
extern crate std;

use soroban_sdk::{Address, String, Vec};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_did_registry::{DidRegistry, DidRegistryClient, ServiceEndpoint};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

fn registry(ctx: &TestContext) -> (Address, DidRegistryClient<'_>) {
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin);
    (contract_id, client)
}

fn text(ctx: &TestContext, value: &str) -> String {
    String::from_str(&ctx.env, value)
}

fn service(ctx: &TestContext, id: &str, kind: &str, uri: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        id: text(ctx, id),
        service_type: text(ctx, kind),
        endpoint: text(ctx, uri),
    }
}

fn services(ctx: &TestContext, entries: &[(&str, &str, &str)]) -> Vec<ServiceEndpoint> {
    let mut list = Vec::new(&ctx.env);
    for (id, kind, uri) in entries {
        list.push_back(service(ctx, id, kind, uri));
    }
    list
}

/// A registry with one DID registered by `ctx.user(0)`.
fn registered(ctx: &TestContext) -> (Address, DidRegistryClient<'_>) {
    let (contract_id, client) = registry(ctx);
    client.register_did(
        ctx.user(0),
        &text(ctx, "did:stellar:alice"),
        &text(ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );
    (contract_id, client)
}

#[test]
fn a_registration_event_carries_the_did_and_the_controller() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (contract_id, _client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let did = text(&ctx, "did:stellar:alice");

    _client.register_did(
        &controller,
        &did,
        &text(&ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );

    // Two fields, each worth asserting on its own: a registration that names
    // the right DID under the wrong controller is still a bug.
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("did")
                .and_then(|value| value.deserialize::<String>())
                == Some(did.clone())
        });
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("controller")
                .and_then(|value| value.deserialize::<Address>())
                == Some(controller.clone())
        });
}

#[test]
fn a_rotation_event_names_both_controllers() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_id, client) = registered(&ctx);
    let previous = ctx.user(0).clone();
    let next = ctx.user(1).clone();

    client.rotate_controller(&text(&ctx, "did:stellar:alice"), &previous, &next);

    EventMatcher::new(&ctx.env)
        .with_topic("rotated")
        .assert_data_matches(|data| {
            data.field("previous")
                .and_then(|field| field.deserialize::<Address>())
                == Some(previous.clone())
        });
    EventMatcher::new(&ctx.env)
        .with_topic("rotated")
        .assert_data_matches(|data| {
            data.field("controller")
                .and_then(|field| field.deserialize::<Address>())
                == Some(next.clone())
        });
}

#[test]
fn a_service_update_event_reports_the_new_endpoint_count() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registered(&ctx);

    client.set_service_endpoints(
        &text(&ctx, "did:stellar:alice"),
        ctx.user(0),
        &services(
            &ctx,
            &[
                (
                    "did:stellar:alice#domain",
                    "LinkedDomains",
                    "https://alice.example",
                ),
                (
                    "did:stellar:alice#inbox",
                    "Messaging",
                    "ipfs://bafybeigdyrzt",
                ),
            ],
        ),
    );

    EventMatcher::new(&ctx.env)
        .with_topic("services")
        .assert_data_matches(|data| {
            data.field("endpoints")
                .and_then(|field| field.deserialize::<u32>())
                == Some(2)
        });
}

#[test]
fn a_revocation_event_carries_the_issuer_and_the_ledger_time() {
    let mut ctx = TestContextBuilder::new().with_users(2).build();
    // The client borrows a clone of the env handle, so advancing the fixture's
    // clock below is visible to it without fighting that borrow.
    let env = ctx.env.clone();
    let (contract_id, _) = registered(&ctx);
    let client = DidRegistryClient::new(&env, &contract_id);
    let issuer = ctx.user(0).clone();
    client.register_credential(
        &issuer,
        &text(&ctx, "vc:alice:degree:1"),
        &text(&ctx, "did:stellar:alice"),
    );

    ctx.set_timestamp(1_700_000_123);
    client.revoke_credential(&issuer, &text(&ctx, "vc:alice:degree:1"));

    EventMatcher::new(&ctx.env)
        .with_topic("revoked")
        .assert_data_matches(|data| {
            data.field("issuer")
                .and_then(|field| field.deserialize::<Address>())
                == Some(issuer.clone())
        });
    EventMatcher::new(&ctx.env)
        .with_topic("revoked")
        .assert_data_matches(|data| {
            data.field("revoked_at")
                .and_then(|field| field.deserialize::<u64>())
                == Some(1_700_000_123)
        });
}

#[test]
fn reading_the_registry_publishes_nothing() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registered(&ctx);
    let did = text(&ctx, "did:stellar:alice");
    client.register_credential(ctx.user(0), &text(&ctx, "vc:alice:degree:1"), &did);

    client.resolve_did(&did);
    client.controller_of(&did);
    client.is_revoked(&text(&ctx, "vc:alice:degree:1"));
    client.admin();

    // None of the reads above published anything, and the matcher's scope is
    // the invocation that just ran.
    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn two_registries_in_one_environment_keep_their_events_apart() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (first_id, first) = registry(&ctx);
    let (second_id, _second) = registry(&ctx);

    first.register_did(
        ctx.user(0),
        &text(&ctx, "did:stellar:alice"),
        &text(&ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );

    EventMatcher::new(&ctx.env)
        .from_contract(&first_id)
        .with_topic("registered")
        .assert_emitted();
    EventMatcher::new(&ctx.env)
        .from_contract(&second_id)
        .with_topic("registered")
        .assert_not_emitted();
}

#[test]
fn a_sequence_of_calls_can_be_asserted_as_one_set() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_id, client) = registry(&ctx);
    let owner = ctx.user(0).clone();
    let next = ctx.user(1).clone();
    let did = text(&ctx, "did:stellar:alice");

    let mut log = EventLog::new(&ctx.env);
    client.register_did(
        &owner,
        &did,
        &text(&ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );
    log.collect();
    client.set_service_endpoints(
        &did,
        &owner,
        &services(
            &ctx,
            &[(
                "did:stellar:alice#domain",
                "LinkedDomains",
                "https://a.example",
            )],
        ),
    );
    log.collect();
    client.rotate_controller(&did, &owner, &next);
    log.collect();
    client.register_credential(&next, &text(&ctx, "vc:alice:1"), &did);
    log.collect();

    // A log keeps every collected call, so the count is over the sequence and
    // not only over the latest invocation the env still holds. Two events end
    // in `registered` — the DID registration and the credential registration —
    // because the matcher filters on any topic position.
    log.matcher().with_topic("registered").assert_count(2);
    log.matcher().with_topic("services").assert_count(1);
    log.matcher().with_topic("rotated").assert_count(1);
    log.matcher().with_topic("issued").assert_not_emitted();
    assert_eq!(log.len(), 4);
    assert_eq!(
        log.topics()[0],
        std::vec![
            std::string::String::from("did"),
            std::string::String::from("registered"),
        ]
    );
    assert_eq!(
        log.topics()[3],
        std::vec![
            std::string::String::from("credential"),
            std::string::String::from("registered"),
        ]
    );
}

#[test]
fn a_registration_records_the_controllers_authorization_with_its_arguments() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();

    client.register_did(
        &controller,
        &text(&ctx, "did:stellar:alice"),
        &text(&ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );

    // Under the builder's default policy the requirement is recorded rather
    // than enforced, so the matcher reads back who authorized which function
    // with which arguments. The DID and the method id are left as `_`: a
    // `String` comparison here would need the same env-converted handle.
    assert_auth_matches!(ctx.env, {
        invoker: controller,
        function: "register_did",
        args: [_, _, _, _],
    });
}

#[test]
fn the_matcher_counts_one_calls_requirement_and_a_reads_none() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registered(&ctx);
    let did = text(&ctx, "did:stellar:alice");

    AuthMatcher::new(&ctx.env).assert_auth_count(1);

    client.resolve_did(&did);

    // A read requires nothing, so the same environment now records nothing at
    // all — the count is per invocation, not per test.
    AuthMatcher::new(&ctx.env).assert_no_auth_required();
}

#[test]
#[should_panic]
fn a_registration_the_controller_never_authorized_is_refused() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);

    // No `initialize` here: `register_did` needs the controller's
    // authorization, so this isolates the refusal of one unapproved call.
    client.register_did(
        ctx.user(0),
        &text(&ctx, "did:stellar:alice"),
        &text(&ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );
}
