//! The generated-input layer: `soroban-testkit-generators` supplies addresses,
//! timestamps and boundary strings, and proptest checks that authorization and
//! document invariants hold for every case.
//!
//! Every authorization case calls the real contract entry point. The environment
//! mocks signatures, so `require_auth` succeeds for whichever signer the case
//! generated; the property under test is the contract's own rule that the signer
//! must also be the document's recorded controller. A test of a standalone
//! helper would not exercise that.

use proptest::prelude::*;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env};
use soroban_testkit_example_did_registry::{DidDocument, DidRegistry, DidRegistryClient};
use soroban_testkit_generators::strategies::{account_address, contract_address, timestamp};

mod common;
use common::{document, document_with, key, service, text};

fn deployed(env: &Env) -> DidRegistryClient<'_> {
    env.mock_all_auths();
    let contract_id = env.register(DidRegistry, ());
    DidRegistryClient::new(env, &contract_id)
}

/// Checks every invariant the contract promises about a stored document.
fn assert_document_invariants(document: &DidDocument, controller: &Address) {
    assert_eq!(&document.controller, controller);
    assert!(!document.verification_methods.is_empty());
    assert!(!document.authentication.is_empty());
    assert!(document.verification_methods.len() <= 8);
    assert!(document.authentication.len() <= 8);
    assert!(document.services.len() <= 8);

    // Method ids and service ids are unique, and every authentication
    // relationship names a method the document actually carries.
    for index in 0..document.verification_methods.len() {
        let method = document.verification_methods.get(index).unwrap();
        assert_eq!(&method.controller, controller);
        for earlier in 0..index {
            assert_ne!(
                document.verification_methods.get(earlier).unwrap().id,
                method.id
            );
        }
    }
    for index in 0..document.authentication.len() {
        let id = document.authentication.get(index).unwrap();
        let mut found = false;
        for method_index in 0..document.verification_methods.len() {
            if document.verification_methods.get(method_index).unwrap().id == id {
                found = true;
            }
        }
        assert!(found, "authentication names a method the document carries");
        for earlier in 0..index {
            assert_ne!(document.authentication.get(earlier).unwrap(), id);
        }
    }
    for index in 0..document.services.len() {
        let service = document.services.get(index).unwrap();
        for earlier in 0..index {
            assert_ne!(document.services.get(earlier).unwrap().id, service.id);
        }
    }
}

proptest! {
    // Each case deploys a contract and makes several calls, so the usual 256
    // cases would dominate a `cargo test` run. 32 keeps every property
    // meaningful and the file quick.
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn only_the_recorded_controller_can_add_a_service(
        signer in account_address(),
        controller in account_address(),
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let signer = signer.to_address(&env);
        let controller = controller.to_address(&env);
        let document = document(&env, "alice", &controller);
        client.register(&controller, &document);

        let endpoint = service(&env, "svc-1", "LinkedDomains", "https://alice.example");
        let result = client.try_add_service(&signer, &document.id, &endpoint);
        let stored = client.resolve(&document.id).unwrap();

        if signer == controller {
            prop_assert!(result.is_ok());
            prop_assert_eq!(stored.services.len(), 1);
        } else {
            prop_assert!(result.is_err());
            prop_assert_eq!(stored.services.len(), 0);
        }
    }

    #[test]
    fn only_the_recorded_controller_can_rotate_a_verification_key(
        signer in account_address(),
        controller in contract_address(),
        seed in any::<u8>(),
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let signer = signer.to_address(&env);
        let controller = controller.to_address(&env);
        let document = document(&env, "alice", &controller);
        client.register(&controller, &document);
        let original_key = document.verification_methods.get(0).unwrap().public_key;

        let result = client.try_rotate_verification_key(
            &signer,
            &document.id,
            &text(&env, "#key-1"),
            &key(&env, seed),
        );
        let stored = client.resolve(&document.id).unwrap();
        let stored_key = stored.verification_methods.get(0).unwrap().public_key;

        if signer == controller {
            prop_assert!(result.is_ok());
            prop_assert_eq!(stored_key, key(&env, seed));
        } else {
            prop_assert!(result.is_err());
            prop_assert_eq!(stored_key, original_key);
        }
    }

    #[test]
    fn only_the_recorded_controller_can_change_the_controller(
        signer in account_address(),
        controller in account_address(),
        successor in account_address(),
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let signer = signer.to_address(&env);
        let controller = controller.to_address(&env);
        let successor = successor.to_address(&env);
        prop_assume!(successor != controller);
        let document = document(&env, "alice", &controller);
        client.register(&controller, &document);

        let result = client.try_transfer_controller(&signer, &document.id, &successor);
        let stored = client.resolve(&document.id).unwrap();

        if signer == controller {
            prop_assert!(result.is_ok());
            prop_assert_eq!(stored.controller, successor);
        } else {
            prop_assert!(result.is_err());
            prop_assert_eq!(stored.controller, controller);
        }
    }

    #[test]
    fn only_the_issuer_did_controller_can_revoke_a_credential(
        signer in account_address(),
        issuer_controller in account_address(),
        credential in "[a-z0-9-]{3,20}",
    ) {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let client = deployed(&env);
        let signer = signer.to_address(&env);
        let issuer_controller = issuer_controller.to_address(&env);
        let issuer = document(&env, "issuer", &issuer_controller);
        client.register(&issuer_controller, &issuer);
        let credential = text(&env, &credential);

        let result = client.try_revoke_credential(
            &signer,
            &issuer.id,
            &credential,
            &text(&env, "superseded"),
        );

        if signer == issuer_controller {
            prop_assert!(result.is_ok());
            prop_assert!(client.is_revoked(&credential));
        } else {
            prop_assert!(result.is_err());
            prop_assert!(!client.is_revoked(&credential));
            prop_assert_eq!(client.revocation_count(), 0);
        }
    }

    #[test]
    fn after_a_legitimate_rotation_the_new_controller_authorizes_and_the_old_one_does_not(
        first in account_address(),
        successor in account_address(),
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let first = first.to_address(&env);
        let successor = successor.to_address(&env);
        prop_assume!(first != successor);
        let document = document(&env, "alice", &first);
        client.register(&first, &document);

        client.transfer_controller(&first, &document.id, &successor);

        let endpoint = service(&env, "svc-1", "LinkedDomains", "https://alice.example");
        // The previous controller signed the rotation, so its authorization was
        // valid a moment ago; it is not valid now.
        prop_assert!(client
            .try_add_service(&first, &document.id, &endpoint)
            .is_err());
        prop_assert!(client
            .try_rotate_verification_key(&first, &document.id, &text(&env, "#key-1"), &key(&env, 3))
            .is_err());

        // The successor authorizes the same calls.
        client.add_service(&successor, &document.id, &endpoint);
        client.rotate_verification_key(
            &successor,
            &document.id,
            &text(&env, "#key-1"),
            &key(&env, 4),
        );

        let stored = client.resolve(&document.id).unwrap();
        assert_document_invariants(&stored, &successor);
        prop_assert_eq!(stored.services.len(), 1);
        prop_assert_eq!(stored.verification_methods.get(0).unwrap().public_key, key(&env, 4));
    }

    #[test]
    fn generated_document_updates_preserve_the_document_invariants(
        methods in 1u32..=6,
        services in 0u32..=6,
        added in 0u32..=2,
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let controller = Address::generate(&env);
        let document = document_with(&env, "alice", &controller, methods, services);
        client.register(&controller, &document);

        for index in 0..added {
            client.add_service(
                &controller,
                &document.id,
                &service(
                    &env,
                    &format!("svc-extra-{index}"),
                    "CredentialRegistry",
                    &format!("https://extra.example/{index}"),
                ),
            );
        }
        if services + added > 0 {
            // With no fixture services the added ones are the only candidates.
            let removable = if services > 0 {
                "svc-0".to_string()
            } else {
                "svc-extra-0".to_string()
            };
            client.remove_service(&controller, &document.id, &text(&env, &removable));
        }
        client.rotate_verification_key(
            &controller,
            &document.id,
            &text(&env, "#key-0"),
            &key(&env, 77),
        );

        let stored = client.resolve(&document.id).expect("the document still resolves");
        assert_document_invariants(&stored, &controller);

        prop_assert_eq!(stored.id, document.id);
        prop_assert_eq!(stored.verification_methods.len(), methods);
        prop_assert_eq!(stored.authentication.len(), methods);
        let expected_services = if services + added > 0 {
            services + added - 1
        } else {
            0
        };
        prop_assert_eq!(stored.services.len(), expected_services);
    }

    #[test]
    fn generated_identifier_shapes_are_accepted_only_with_the_prefix_and_charset(
        prefix in prop_oneof![
            Just("did:soroban:"),
            Just("did:example:"),
            Just("soroban:"),
            Just(""),
        ],
        suffix in "[a-z0-9.-]{0,20}",
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let controller = Address::generate(&env);
        let mut document = document(&env, "alice", &controller);
        document.id = text(&env, &format!("{prefix}{suffix}"));

        // Only the exact `did:soroban:` shape with a non-empty name is
        // accepted; a prefix that merely resembles it is not.
        let registered = client.try_register(&controller, &document).is_ok();
        let expected = prefix == "did:soroban:" && !suffix.is_empty();
        prop_assert_eq!(registered, expected);
        prop_assert_eq!(client.document_count(), if expected { 1 } else { 0 });
    }

    #[test]
    fn generated_identifiers_with_uppercase_characters_are_refused(suffix in "[A-Z][a-zA-Z0-9]{0,12}") {
        let env = Env::default();
        let client = deployed(&env);
        let controller = Address::generate(&env);
        let mut document = document(&env, "alice", &controller);
        document.id = text(&env, &format!("did:soroban:{suffix}"));

        prop_assert!(client.try_register(&controller, &document).is_err());
    }

    #[test]
    fn documents_authenticating_a_generated_unknown_key_are_refused(auth_id in "[a-z]{4,12}") {
        let env = Env::default();
        let client = deployed(&env);
        let controller = Address::generate(&env);
        let mut document = document(&env, "alice", &controller);
        let mut authentication = soroban_sdk::Vec::new(&env);
        authentication.push_back(text(&env, &auth_id));
        document.authentication = authentication;

        prop_assert!(client.try_register(&controller, &document).is_err());
    }

    #[test]
    fn a_registration_is_unique_for_every_generated_suffix(suffix in "[a-z0-9-]{1,16}") {
        let env = Env::default();
        let client = deployed(&env);
        let controller = Address::generate(&env);
        let document = document(&env, &suffix, &controller);

        client.register(&controller, &document);
        let second = client.try_register(&controller, &document);

        prop_assert!(second.is_err());
        prop_assert_eq!(client.document_count(), 1);
        prop_assert_eq!(client.resolve(&document.id), Some(document));
    }

    #[test]
    fn a_generated_revocation_is_idempotent(
        credential in "[a-z0-9-]{3,20}",
        first_at in timestamp(),
        second_at in timestamp(),
    ) {
        let env = Env::default();
        env.ledger().set_timestamp(first_at);
        let client = deployed(&env);
        let controller = Address::generate(&env);
        let issuer = document(&env, "issuer", &controller);
        client.register(&controller, &issuer);
        let credential = text(&env, &credential);

        let first = client.revoke_credential(
            &controller,
            &issuer.id,
            &credential,
            &text(&env, "superseded"),
        );
        env.ledger().set_timestamp(second_at);
        let second = client.revoke_credential(
            &controller,
            &issuer.id,
            &credential,
            &text(&env, "another reason"),
        );

        // The first revocation fixes both the time and the reason, whichever
        // order the two generated timestamps arrived in.
        prop_assert_eq!(second.clone(), first.clone());
        prop_assert_eq!(second.revoked_at, first_at);
        prop_assert_eq!(client.revocation_count(), 1);
        prop_assert_eq!(client.revocation_status(&credential), Some(first));
    }

    #[test]
    fn an_account_and_a_contract_controller_are_both_accepted(
        account in account_address(),
        contract in contract_address(),
    ) {
        let env = Env::default();
        let client = deployed(&env);
        let account = account.to_address(&env);
        let contract = contract.to_address(&env);

        let first = document(&env, "account-held", &account);
        client.register(&account, &first);
        let moved = client.transfer_controller(&account, &first.id, &contract);

        // Soroban authorizes both address formats the same way, and the
        // generator hands over one of each, so the registry has to treat them
        // as interchangeable controllers.
        prop_assert_eq!(moved.controller, contract.clone());
        let endpoint = service(&env, "svc-1", "LinkedDomains", "https://contract.example");
        client.add_service(&contract, &first.id, &endpoint);
        prop_assert_eq!(client.resolve(&first.id).unwrap().services.len(), 1);
    }
}
