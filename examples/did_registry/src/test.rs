extern crate std;

// The fixture layer on its own: what `TestContext` hands a registry test, how
// the builder shapes it, and what a document looks like once it is stored. The
// other three files in this package each take one of the remaining crates.
use soroban_sdk::{Address, String, Vec};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

use crate::{DidRegistry, DidRegistryClient, ServiceEndpoint};

/// A context with `users` controllers and a mocked-auth policy, which is what a
/// registry test usually wants: the contract's `require_auth` calls succeed
/// without the test describing them.
fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

/// Registers the registry in `ctx` and initializes it with the fixture's admin.
///
/// Registration belongs to the env, so this is the step a test repeats after
/// `TestContext::reset`.
fn registry(ctx: &TestContext) -> (Address, DidRegistryClient<'_>) {
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin);
    (contract_id, client)
}

/// A `soroban_sdk::String` owned by `ctx`.
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

#[test]
fn initialize_records_the_operator() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    assert_eq!(client.admin(), ctx.admin);
}

#[test]
#[should_panic(expected = "registry already initialized")]
fn initialize_twice_points_at_the_call_that_already_happened() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.initialize(&ctx.admin);
}

#[test]
#[should_panic(expected = "registry not initialized")]
fn reading_an_operator_before_initialize_fails_loudly() {
    let ctx = context(1);
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);

    client.admin();
}

#[test]
fn registering_a_did_records_the_document_and_its_verification_method() {
    let mut ctx = context(1);
    ctx.set_timestamp(1_700_000_000);
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let did = text(&ctx, "did:stellar:alice");

    let document = client.register_did(
        &controller,
        &did,
        &text(&ctx, "did:stellar:alice#key-1"),
        &Vec::new(&ctx.env),
    );

    // The fixture's identities are the controllers, and the clock is the ledger
    // clock the test chose, so both numbers are stated rather than read back.
    assert_eq!(document.id, did);
    assert_eq!(document.controller, controller);
    assert_eq!(document.verification_method.controller, controller);
    assert_eq!(
        document.verification_method.id,
        text(&ctx, "did:stellar:alice#key-1")
    );
    assert_eq!(document.version, 1);
    assert_eq!(document.updated_at, 1_700_000_000);
    assert!(document.services.is_empty());
    assert_eq!(client.resolve_did(&did), Some(document));
}

#[test]
#[should_panic(expected = "DID is already registered")]
fn a_duplicate_registration_is_refused() {
    let ctx = context(2);
    let (_id, client) = registry(&ctx);
    let did = text(&ctx, "did:stellar:alice");
    let method = text(&ctx, "did:stellar:alice#key-1");
    client.register_did(ctx.user(0), &did, &method, &Vec::new(&ctx.env));

    // A second controller may not take over an identifier that exists: control
    // moves through rotation, which the current controller authorizes.
    client.register_did(ctx.user(1), &did, &method, &Vec::new(&ctx.env));
}

#[test]
#[should_panic(expected = "DID must use the did:stellar: method")]
fn a_did_without_the_stellar_method_is_refused() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.register_did(
        ctx.user(0),
        &text(&ctx, "did:example:alice"),
        &text(&ctx, "did:example:alice#key-1"),
        &Vec::new(&ctx.env),
    );
}

#[test]
#[should_panic(expected = "DID must name a method-specific identifier")]
fn a_did_with_an_empty_method_specific_identifier_is_refused() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.register_did(
        ctx.user(0),
        &text(&ctx, "did:stellar:"),
        &text(&ctx, "did:stellar:#key-1"),
        &Vec::new(&ctx.env),
    );
}

#[test]
#[should_panic(expected = "DID may only contain letters, digits")]
fn a_did_with_invalid_characters_is_refused() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.register_did(
        ctx.user(0),
        &text(&ctx, "did:stellar:ali ce"),
        &text(&ctx, "did:stellar:ali ce#key-1"),
        &Vec::new(&ctx.env),
    );
}

#[test]
#[should_panic(expected = "verification method id must be the DID followed by '#'")]
fn a_verification_method_outside_the_did_is_refused() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    // The method id starts with the DID but names no fragment. W3C DID Core
    // allows a verification method without an id in some profiles; this
    // contract does not, because the id is how rotation and resolution point at
    // a key.
    client.register_did(
        ctx.user(0),
        &text(&ctx, "did:stellar:alice"),
        &text(&ctx, "did:stellar:alice"),
        &Vec::new(&ctx.env),
    );
}

#[test]
fn resolving_a_missing_did_answers_none() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);
    let missing = text(&ctx, "did:stellar:nobody");

    assert_eq!(client.resolve_did(&missing), None);
    assert_eq!(client.controller_of(&missing), None);
    assert_eq!(client.document_version(&missing), None);
    assert_eq!(
        client.service_endpoint(&missing, &text(&ctx, "did:stellar:nobody#svc")),
        None
    );
}

#[test]
fn reset_keeps_the_controllers_and_empties_the_registry() {
    let mut ctx = context(1);
    let (_id, client) = registry(&ctx);
    let did = text(&ctx, "did:stellar:alice");
    let method = text(&ctx, "did:stellar:alice#key-1");
    client.register_did(ctx.user(0), &did, &method, &Vec::new(&ctx.env));
    assert!(client.resolve_did(&did).is_some());

    // Same identities, brand-new ledger: the controllers survive, the storage
    // does not, so the registry has to be registered and initialized again.
    ctx.reset();
    let (_id, client) = registry(&ctx);

    // The fixture carries the identities across the reset, but every object
    // handle belongs to the env that minted it: the DID is rebuilt in the new
    // env before it is asked about.
    let did = text(&ctx, "did:stellar:alice");
    assert_eq!(client.resolve_did(&did), None);
    assert_eq!(client.admin(), ctx.admin);
}

#[test]
fn add_user_grows_the_context_for_the_next_controller() {
    let mut ctx = context(1);
    let late = ctx.add_user().clone();
    let (_id, client) = registry(&ctx);

    let document = client.register_did(
        &late,
        &text(&ctx, "did:stellar:bob"),
        &text(&ctx, "did:stellar:bob#key-1"),
        &Vec::new(&ctx.env),
    );

    assert_eq!(ctx.users.len(), 2);
    assert_eq!(document.controller, late);
}

#[test]
#[should_panic(expected = "user index out of bounds: index is 2 but context holds 2 users")]
fn asking_for_a_controller_that_was_never_built_points_at_with_users() {
    let ctx = context(2);

    ctx.user(2);
}

#[test]
fn registering_with_initial_service_endpoints_exposes_them() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);
    let did = text(&ctx, "did:stellar:alice");

    let document = client.register_did(
        ctx.user(0),
        &did,
        &text(&ctx, "did:stellar:alice#key-1"),
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

    assert_eq!(document.services.len(), 2);
    assert_eq!(
        client.service_endpoint(&did, &text(&ctx, "did:stellar:alice#domain")),
        Some(service(
            &ctx,
            "did:stellar:alice#domain",
            "LinkedDomains",
            "https://alice.example"
        ))
    );
}

#[test]
#[should_panic(expected = "service endpoint URI must be an https:// or ipfs:// URI")]
fn registering_with_an_unsupported_service_uri_scheme_is_refused() {
    let ctx = context(1);
    let (_id, client) = registry(&ctx);

    client.register_did(
        ctx.user(0),
        &text(&ctx, "did:stellar:alice"),
        &text(&ctx, "did:stellar:alice#key-1"),
        &services(&ctx, &[("did:stellar:alice#bad", "Messaging", "ftp://x")]),
    );
}
