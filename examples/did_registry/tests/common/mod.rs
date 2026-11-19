// Shared fixtures for the four integration-test files in this package.
//
// Cargo compiles each `tests/*.rs` file as its own crate, so a helper used by
// two of them lives in this directory and is pulled in with `mod common;`.
// Each file uses a different subset, hence the module-level `dead_code`
// allowance: an unused fixture is not a warning about the test that did not
// need it.
#![allow(dead_code)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, BytesN, Env, String};
use soroban_testkit_example_did_registry::{
    DidDocument, DidRegistry, DidRegistryClient, ServiceEndpoint, VerificationMethod,
};

/// A `soroban_sdk::String` owned by `env`.
pub fn text(env: &Env, value: &str) -> String {
    String::from_str(env, value)
}

/// A deterministic 32-byte Ed25519 public key.
///
/// The registry only accepts raw keys, so a fixture is a seed byte repeated,
/// which makes two keys from different seeds impossible to confuse.
pub fn key(env: &Env, seed: u8) -> BytesN<32> {
    BytesN::from_array(env, &[seed; 32])
}

/// A DID identifier in the subset the registry accepts.
pub fn did(env: &Env, name: &str) -> String {
    String::from_str(env, &format!("did:soroban:{name}"))
}

/// A verification method controlled by `controller`, with a key derived from
/// the method's position.
pub fn method(env: &Env, id: &str, controller: &Address, seed: u8) -> VerificationMethod {
    VerificationMethod {
        id: text(env, id),
        controller: controller.clone(),
        public_key: key(env, seed),
    }
}

/// A service endpoint the registry accepts: an https URI with a type.
pub fn service(env: &Env, id: &str, service_type: &str, endpoint: &str) -> ServiceEndpoint {
    ServiceEndpoint {
        id: text(env, id),
        service_type: text(env, service_type),
        endpoint: text(env, endpoint),
    }
}

/// A minimal valid document: one verification method, one authentication
/// relationship, no service endpoints.
pub fn document(env: &Env, name: &str, controller: &Address) -> DidDocument {
    let mut authentication = soroban_sdk::Vec::new(env);
    authentication.push_back(text(env, "#key-1"));

    DidDocument {
        id: did(env, name),
        controller: controller.clone(),
        verification_methods: {
            let mut methods = soroban_sdk::Vec::new(env);
            methods.push_back(method(env, "#key-1", controller, 1));
            methods
        },
        authentication,
        services: soroban_sdk::Vec::new(env),
    }
}

/// A document with `method_count` verification methods and `service_count`
/// service endpoints, all authenticated, which is the shape the size tests
/// grow.
pub fn document_with(
    env: &Env,
    name: &str,
    controller: &Address,
    method_count: u32,
    service_count: u32,
) -> DidDocument {
    let mut verification_methods = soroban_sdk::Vec::new(env);
    let mut authentication = soroban_sdk::Vec::new(env);
    for index in 0..method_count {
        let id = format!("#key-{index}");
        let seed = (index + 1) as u8;
        verification_methods.push_back(method(env, &id, controller, seed));
        authentication.push_back(text(env, &id));
    }

    let mut services = soroban_sdk::Vec::new(env);
    for index in 0..service_count {
        services.push_back(service(
            env,
            &format!("svc-{index}"),
            "LinkedDomains",
            &format!("https://example.com/{name}/{index}"),
        ));
    }

    DidDocument {
        id: did(env, name),
        controller: controller.clone(),
        verification_methods,
        authentication,
        services,
    }
}

/// A fresh environment with mocked authorizations and a registered registry.
///
/// The client borrows the environment, so a test builds it from the returned
/// contract id rather than receiving one.
pub fn setup() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(DidRegistry, ());
    (env, contract_id)
}

/// The client for a contract id in `env`.
pub fn client<'a>(env: &'a Env, contract_id: &Address) -> DidRegistryClient<'a> {
    DidRegistryClient::new(env, contract_id)
}

/// A fresh signer address in `env`.
pub fn signer(env: &Env) -> Address {
    Address::generate(env)
}

/// Identifiers the registry must reject, as plain strings so a test can rebuild
/// each one in its own environment.
pub fn invalid_did_texts() -> std::vec::Vec<std::string::String> {
    std::vec![
        std::string::String::new(),
        "did:soroban:".to_string(),
        "did:example:alice".to_string(),
        "soroban:alice".to_string(),
        "did:soroban:Alice".to_string(),
        "did:soroban:alice/extra".to_string(),
        "did:soroban:alice extra".to_string(),
        format!("did:soroban:{}", "a".repeat(96)),
    ]
}
