//! The fixture layer: what a valid DID document looks like, which documents are
//! malformed, and how the four fixture shapes (small, medium, large, invalid)
//! behave against the real contract.
//!
//! This file doubles as the shared support module for its three siblings: Cargo
//! compiles each `tests/*.rs` as a separate crate, so the `common` module next
//! to it holds the builders, and each file pulls them in with `mod common;`.

mod common;

use common::*;
use soroban_sdk::Address;
use soroban_testkit_example_did_registry::{DidRegistry, DidRegistryClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

/// Registers the registry inside a fixture context, which is what the fixture
/// layer contributes to this package: identities and an environment in one
/// builder call.
fn registry(ctx: &TestContext) -> (Address, DidRegistryClient<'_>) {
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);
    (contract_id, client)
}

#[test]
fn a_context_registers_a_document_for_its_first_user() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let document = document(&ctx.env, "alice", &controller);

    client.register(&controller, &document);

    let resolved = client.resolve(&document.id).expect("the document resolves");
    assert_eq!(resolved.controller, controller);
    assert_eq!(resolved.id, document.id);
    assert_eq!(client.controller_of(&document.id), Some(controller));
    assert_eq!(client.document_count(), 1);
}

#[test]
fn the_small_medium_and_large_fixtures_all_register() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();

    for (name, methods, services) in [("small", 1, 0), ("medium", 4, 3), ("large", 8, 8)] {
        let document = document_with(&ctx.env, name, &controller, methods, services);
        client.register(&controller, &document);

        let resolved = client.resolve(&document.id).expect("the document resolves");
        assert_eq!(resolved.verification_methods.len(), methods);
        assert_eq!(resolved.authentication.len(), methods);
        assert_eq!(resolved.services.len(), services);
    }

    assert_eq!(client.document_count(), 3);
}

#[test]
fn documents_without_a_verification_method_are_refused() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let mut document = document(&ctx.env, "alice", &controller);
    document.verification_methods = soroban_sdk::Vec::new(&ctx.env);

    assert!(client.try_register(&controller, &document).is_err());
    assert_eq!(client.document_count(), 0);
}

#[test]
fn documents_authenticating_an_unknown_method_are_refused() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let mut document = document(&ctx.env, "alice", &controller);
    let mut authentication = soroban_sdk::Vec::new(&ctx.env);
    authentication.push_back(text(&ctx.env, "#missing"));
    document.authentication = authentication;

    assert!(client.try_register(&controller, &document).is_err());
    assert!(!client.is_registered(&did(&ctx.env, "alice")));
}

#[test]
fn documents_with_a_duplicate_method_id_are_refused() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let mut document = document(&ctx.env, "alice", &controller);
    document
        .verification_methods
        .push_back(method(&ctx.env, "#key-1", &controller, 2));

    assert!(client.try_register(&controller, &document).is_err());
}

#[test]
fn documents_whose_method_names_another_controller_are_refused() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let impostor = ctx.user(1).clone();
    let mut document = document(&ctx.env, "alice", &controller);
    document.verification_methods = {
        let mut methods = soroban_sdk::Vec::new(&ctx.env);
        methods.push_back(method(&ctx.env, "#key-1", &impostor, 1));
        methods
    };

    assert!(client.try_register(&controller, &document).is_err());
}

#[test]
fn documents_with_a_duplicate_service_id_are_refused() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (_id, client) = registry(&ctx);
    let controller = ctx.user(0).clone();
    let mut document = document(&ctx.env, "alice", &controller);
    document.services.push_back(service(
        &ctx.env,
        "svc-1",
        "LinkedDomains",
        "https://a.example",
    ));
    document.services.push_back(service(
        &ctx.env,
        "svc-1",
        "LinkedDomains",
        "https://b.example",
    ));

    assert!(client.try_register(&controller, &document).is_err());
}

#[test]
fn malformed_did_identifiers_are_refused() {
    // One environment per case: the first refusal must not hide the rest, and a
    // fresh env keeps the registry empty for the next identifier.
    for identifier in invalid_did_texts() {
        let (env, contract_id) = setup();
        let client = client(&env, &contract_id);
        let controller = signer(&env);
        let mut document = document(&env, "alice", &controller);
        document.id = text(&env, &identifier);

        assert!(
            client.try_register(&controller, &document).is_err(),
            "{identifier:?} should not be a registerable DID"
        );
        assert_eq!(client.document_count(), 0);
    }
}
