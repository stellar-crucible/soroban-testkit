//! The assertion layer: the contract's observable behaviour expressed through
//! `soroban-testkit-assert`.
//!
//! Every test here asks what the registry emitted, what it recorded as
//! authorized, or which error it raised — not how much it cost. The three
//! sibling files cover fixtures, budgets and generated inputs.

mod common;

use common::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, String};
use soroban_testkit_assert::assert_auth_matches;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_example_did_registry::{DidRegistry, DidRegistryClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;

fn deployed() -> (Env, Address) {
    let (env, contract_id) = setup();
    (env, contract_id)
}

fn register(client: &DidRegistryClient<'_>, env: &Env, controller: &Address, name: &str) -> String {
    let document = document(env, name, controller);
    client.register(controller, &document);
    document.id
}

#[test]
fn a_registration_event_carries_the_did_and_the_controller() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "alice", &controller);

    client.register(&controller, &document);

    EventMatcher::new(&env)
        .from_contract(&contract_id)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("did")
                .and_then(|value| value.deserialize::<String>())
                == Some(document.id.clone())
        });
    EventMatcher::new(&env)
        .with_topic("registered")
        .assert_data_matches(|data| {
            data.field("controller")
                .and_then(|value| value.deserialize::<Address>())
                == Some(controller.clone())
        });
}

#[test]
fn a_resolved_document_keeps_every_field_it_was_registered_with() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let registered = document_with(&env, "alice", &controller, 3, 2);

    client.register(&controller, &registered);
    let resolved = client
        .resolve(&registered.id)
        .expect("register stores the document");

    assert_eq!(resolved, registered);
    assert_eq!(resolved.services.len(), 2);
    assert_eq!(resolved.authentication.len(), 3);
}

#[test]
fn resolving_an_unknown_did_returns_nothing() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);

    assert_eq!(client.resolve(&did(&env, "nobody")), None);
    assert_eq!(client.controller_of(&did(&env, "nobody")), None);
    assert!(!client.is_registered(&did(&env, "nobody")));
    assert_eq!(client.document_count(), 0);
}

#[test]
fn registering_the_same_did_twice_is_refused() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "alice", &controller);

    client.register(&controller, &document);
    assert!(client.try_register(&controller, &document).is_err());

    // The refusal left the first registration in place rather than overwriting
    // or clearing it.
    assert_eq!(client.document_count(), 1);
    assert_eq!(client.resolve(&document.id), Some(document));
}

#[test]
fn transferring_control_moves_the_document_and_every_method() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let successor = signer(&env);
    let document = document_with(&env, "alice", &controller, 3, 1);
    client.register(&controller, &document);

    let moved = client.transfer_controller(&controller, &document.id, &successor);

    // The matcher reads the invocation that just ran, so the event is asserted
    // before any read replaces that scope.
    EventMatcher::new(&env)
        .with_topic("controller_rotated")
        .assert_data_matches(|data| {
            data.field("to")
                .and_then(|value| value.deserialize::<Address>())
                == Some(successor.clone())
        });

    assert_eq!(moved.controller, successor.clone());
    for index in 0..moved.verification_methods.len() {
        let method = moved.verification_methods.get(index).unwrap();
        assert_eq!(
            method.controller, successor,
            "every method follows its document"
        );
    }
    assert_eq!(client.controller_of(&document.id), Some(successor.clone()));
}

#[test]
fn a_controller_rotation_keeps_the_document_resolvable_under_the_new_controller() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let successor = signer(&env);
    let document = document_with(&env, "alice", &controller, 2, 2);
    client.register(&controller, &document);
    client.transfer_controller(&controller, &document.id, &successor);

    let moved = client
        .resolve(&document.id)
        .expect("a rotated document still resolves");

    // Only the controller fields moved. The identifiers, the authentication
    // relationships and the service endpoints are exactly what was registered.
    assert_eq!(moved.id, document.id);
    assert_eq!(moved.authentication, document.authentication);
    assert_eq!(moved.services, document.services);
}

#[test]
fn a_key_rotation_replaces_the_key_and_keeps_the_authentication_relationship() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "alice", &controller);
    client.register(&controller, &document);

    let replacement = key(&env, 42);
    let rotated = client.rotate_verification_key(
        &controller,
        &document.id,
        &text(&env, "#key-1"),
        &replacement,
    );

    let method = rotated.verification_methods.get(0).unwrap();
    assert_eq!(method.public_key, replacement);
    assert_eq!(method.id, text(&env, "#key-1"));
    // Authentication names the method id, so it now resolves to the new key
    // without the relationship being rewritten.
    assert_eq!(rotated.authentication, document.authentication);

    EventMatcher::new(&env)
        .with_topic("key_rotated")
        .assert_data_matches(|data| {
            data.field("key_id")
                .and_then(|value| value.deserialize::<String>())
                == Some(text(&env, "#key-1"))
        });
}

#[test]
fn rotating_a_method_the_document_does_not_carry_is_refused() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "alice", &controller);
    client.register(&controller, &document);

    assert!(client
        .try_rotate_verification_key(
            &controller,
            &document.id,
            &text(&env, "#missing"),
            &key(&env, 9)
        )
        .is_err());
    assert_eq!(client.resolve(&document.id), Some(document));
}

#[test]
fn a_service_endpoint_is_added_without_touching_the_rest_of_the_document() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document_with(&env, "alice", &controller, 2, 1);
    client.register(&controller, &document);

    let added = service(&env, "svc-new", "LinkedDomains", "https://alice.example");
    let updated = client.add_service(&controller, &document.id, &added);

    assert_eq!(updated.services.len(), 2);
    assert_eq!(updated.verification_methods, document.verification_methods);
    assert_eq!(updated.authentication, document.authentication);
    assert_eq!(updated.services.get(1).unwrap(), added);

    EventMatcher::new(&env)
        .with_topic("service_added")
        .assert_data_matches(|data| {
            data.field("service_id")
                .and_then(|value| value.deserialize::<String>())
                == Some(text(&env, "svc-new"))
        });
}

#[test]
fn updating_a_service_replaces_only_that_endpoint() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document_with(&env, "alice", &controller, 1, 2);
    client.register(&controller, &document);

    let replacement = service(
        &env,
        "svc-0",
        "CredentialRegistry",
        "https://registry.example",
    );
    let updated = client.update_service(&controller, &document.id, &replacement);

    assert_eq!(updated.services.get(0).unwrap(), replacement);
    assert_eq!(
        updated.services.get(1).unwrap(),
        document.services.get(1).unwrap()
    );
    assert_eq!(updated.verification_methods, document.verification_methods);
}

#[test]
fn removing_a_service_keeps_the_other_endpoints() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document_with(&env, "alice", &controller, 1, 3);
    client.register(&controller, &document);

    let removed = client.remove_service(&controller, &document.id, &text(&env, "svc-1"));

    assert_eq!(removed.services.len(), 2);
    assert_eq!(
        removed.services.get(0).unwrap(),
        document.services.get(0).unwrap()
    );
    assert_eq!(
        removed.services.get(1).unwrap(),
        document.services.get(2).unwrap()
    );

    EventMatcher::new(&env)
        .with_topic("service_removed")
        .assert_data_matches(|data| {
            data.field("service_id")
                .and_then(|value| value.deserialize::<String>())
                == Some(text(&env, "svc-1"))
        });
}

#[test]
fn updating_or_removing_an_unknown_service_is_refused() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "alice", &controller);
    client.register(&controller, &document);

    let unknown = service(
        &env,
        "svc-missing",
        "LinkedDomains",
        "https://missing.example",
    );
    assert!(client
        .try_update_service(&controller, &document.id, &unknown)
        .is_err());
    assert!(client
        .try_remove_service(&controller, &document.id, &text(&env, "svc-missing"))
        .is_err());
    assert_eq!(client.resolve(&document.id), Some(document));
}

#[test]
fn a_controller_that_was_rotated_away_can_no_longer_modify_anything() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let successor = signer(&env);
    let document = document(&env, "alice", &controller);
    client.register(&controller, &document);
    client.transfer_controller(&controller, &document.id, &successor);

    let endpoint = service(&env, "svc-1", "LinkedDomains", "https://alice.example");

    // The old address still passes `require_auth` under this environment's
    // mocked policy; what refuses it is the contract's controller check, which
    // is the authorization that matters after a rotation.
    assert!(client
        .try_add_service(&controller, &document.id, &endpoint)
        .is_err());
    assert!(client
        .try_rotate_verification_key(
            &controller,
            &document.id,
            &text(&env, "#key-1"),
            &key(&env, 7)
        )
        .is_err());
    assert!(client
        .try_transfer_controller(&controller, &document.id, &controller)
        .is_err());

    // The new controller carries the same calls without complaint.
    client.add_service(&successor, &document.id, &endpoint);
    assert_eq!(client.resolve(&document.id).unwrap().services.len(), 1);
}

#[test]
fn revoking_a_credential_records_the_issuer_the_time_and_the_reason() {
    let (env, contract_id) = deployed();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let issuer = register(&client, &env, &controller, "issuer");
    let credential = text(&env, "urn:uuid:credential-1");

    let record =
        client.revoke_credential(&controller, &issuer, &credential, &text(&env, "superseded"));

    // Asserted before the reads below, because a matcher reads the invocation
    // that just ran and a read replaces it.
    EventMatcher::new(&env)
        .with_topic("revoked")
        .assert_data_matches(|data| {
            data.field("credential_id")
                .and_then(|value| value.deserialize::<String>())
                == Some(text(&env, "urn:uuid:credential-1"))
        });

    assert_eq!(record.credential_id, credential);
    assert_eq!(record.issuer, issuer);
    assert_eq!(record.revoked_at, 1_700_000_000);
    assert_eq!(record.reason, text(&env, "superseded"));
    assert!(client.is_revoked(&credential));
    assert_eq!(client.revocation_count(), 1);
}

#[test]
fn a_repeated_revocation_returns_the_first_record_and_publishes_nothing() {
    let (env, contract_id) = deployed();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let issuer = register(&client, &env, &controller, "issuer");
    let credential = text(&env, "urn:uuid:credential-1");

    let first =
        client.revoke_credential(&controller, &issuer, &credential, &text(&env, "superseded"));

    env.ledger().set_timestamp(1_800_000_000);
    let second = client.revoke_credential(
        &controller,
        &issuer,
        &credential,
        &text(&env, "a different reason"),
    );

    // First write wins: neither the timestamp nor the reason moved, and the
    // call published no second event.
    assert_eq!(second, first);
    assert_eq!(second.revoked_at, 1_700_000_000);
    assert_eq!(client.revocation_count(), 1);
    EventMatcher::new(&env)
        .with_topic("revoked")
        .assert_not_emitted();
}

#[test]
fn revocation_status_is_empty_until_a_credential_is_revoked() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let credential = text(&env, "urn:uuid:unknown");

    assert_eq!(client.revocation_status(&credential), None);
    assert!(!client.is_revoked(&credential));
    assert_eq!(client.revocation_count(), 0);
    EventMatcher::new(&env).assert_not_emitted();
}

#[test]
fn one_controller_cannot_revoke_on_another_issuers_behalf() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let issuer_controller = signer(&env);
    let attacker = signer(&env);
    let issuer = register(&client, &env, &issuer_controller, "issuer");
    register(&client, &env, &attacker, "attacker");

    assert!(client
        .try_revoke_credential(
            &attacker,
            &issuer,
            &text(&env, "urn:uuid:credential-1"),
            &text(&env, "not mine"),
        )
        .is_err());
    assert!(!client.is_revoked(&text(&env, "urn:uuid:credential-1")));

    // A credential cannot be reassigned to a second issuer either: the first
    // revocation is the only one that exists.
    client.revoke_credential(
        &issuer_controller,
        &issuer,
        &text(&env, "urn:uuid:credential-1"),
        &text(&env, "superseded"),
    );
    assert!(client
        .try_revoke_credential(
            &attacker,
            &did(&env, "attacker"),
            &text(&env, "urn:uuid:credential-1"),
            &text(&env, "mine now"),
        )
        .is_err());
}

#[test]
fn a_registration_records_the_controllers_authorization() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);
    let controller = ctx.user(0).clone();
    let document = document(&ctx.env, "alice", &controller);

    client.register(&controller, &document);

    assert_auth_matches!(ctx.env, {
        invoker: controller,
        function: "register",
        args: [_, _],
    });
    AuthMatcher::new(&ctx.env).assert_auth_count(1);
}

#[test]
fn reads_require_and_publish_nothing() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let contract_id = ctx.env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&ctx.env, &contract_id);
    let controller = ctx.user(0).clone();
    let document = document(&ctx.env, "alice", &controller);
    client.register(&controller, &document);

    client.resolve(&document.id);
    client.controller_of(&document.id);
    client.is_registered(&document.id);
    client.document_count();
    client.revocation_status(&text(&ctx.env, "urn:uuid:none"));

    AuthMatcher::new(&ctx.env).assert_no_auth_required();
    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn a_sequence_of_controller_calls_emits_one_event_each() {
    let (env, contract_id) = deployed();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "alice", &controller);

    let mut log = EventLog::new(&env);
    client.register(&controller, &document);
    log.collect();
    client.add_service(
        &controller,
        &document.id,
        &service(&env, "svc-1", "LinkedDomains", "https://alice.example"),
    );
    log.collect();
    client.remove_service(&controller, &document.id, &text(&env, "svc-1"));
    log.collect();

    log.matcher().with_topic("registered").assert_count(1);
    log.matcher().with_topic("service_added").assert_count(1);
    log.matcher().with_topic("service_removed").assert_count(1);
    log.matcher()
        .with_topic("service_updated")
        .assert_not_emitted();
    assert_eq!(log.len(), 3);
}

#[test]
#[should_panic]
fn registration_without_the_controllers_authorization_is_refused() {
    let env = Env::default();
    let contract_id = env.register(DidRegistry, ());
    let client = DidRegistryClient::new(&env, &contract_id);
    let controller = Address::generate(&env);
    let document = document(&env, "alice", &controller);

    // No mock policy: `require_auth` is enforced by the host, so this isolates
    // the refusal of one unsigned call.
    client.register(&controller, &document);
}
