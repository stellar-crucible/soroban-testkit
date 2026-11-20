//! One crate per file: this suite drives the registry through
//! `soroban-testkit-core`, whose two jobs here are naming what a call cost and
//! how many bytes a document occupies in the ledger.
//!
//! The harness is a bare `Env`, so nothing in these tests reaches for another
//! testkit crate — `src/test.rs` is where the fixture builder is demonstrated.

extern crate std;

use std::string::String as StdString;
use std::vec::Vec as StdVec;

use soroban_sdk::testutils::Address as _;
use soroban_sdk::{Address, Env, String, Vec};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::footprint::FootprintSnapshot;
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
use soroban_testkit_example_did_registry::{DidRegistry, DidRegistryClient, ServiceEndpoint};

fn deployed() -> (Env, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(DidRegistry, ());
    let admin = Address::generate(&env);
    DidRegistryClient::new(&env, &contract).initialize(&admin);
    (env, contract)
}

fn client<'a>(env: &'a Env, contract: &Address) -> DidRegistryClient<'a> {
    DidRegistryClient::new(env, contract)
}

fn text(env: &Env, value: &str) -> String {
    String::from_str(env, value)
}

/// `count` endpoints whose URIs are `8 + padding` bytes long.
fn endpoints(env: &Env, did: &str, count: u32, padding: usize) -> Vec<ServiceEndpoint> {
    let mut list = Vec::new(env);
    for index in 0..count {
        list.push_back(ServiceEndpoint {
            id: String::from_str(env, &format!("{did}#svc-{index}")),
            service_type: text(env, "LinkedDomains"),
            endpoint: String::from_str(env, &format!("https://{}", "a".repeat(padding))),
        });
    }
    list
}

/// The XDR bytes one registered document adds to the contract's footprint.
///
/// The registry is deployed and initialized first, so the baseline includes the
/// instance `admin` entry and the difference is exactly the persistent entry
/// the registration opened: its key and its value, the unit rent is charged in.
fn document_bytes(count: u32, padding: usize) -> u64 {
    let (env, contract) = deployed();
    let before = FootprintSnapshot::capture(&env, &contract).total_bytes();
    client(&env, &contract).register_did(
        &Address::generate(&env),
        &text(&env, "did:stellar:measured"),
        &text(&env, "did:stellar:measured#key-1"),
        &endpoints(&env, "did:stellar:measured", count, padding),
    );
    FootprintSnapshot::capture(&env, &contract).total_bytes() - before
}

#[test]
fn a_registration_stays_inside_its_transaction_ceiling() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);

    // A ceiling is transaction headroom, not this call's measured cost: a
    // contract registered natively is metered without the VM's own costs, so a
    // registration reads tens of thousands of instructions and a network would
    // bill more.
    soroban_testkit_core::budget_guard!(
        &env,
        "register_did",
        { cpu_max: 20_000_000, mem_max: 20_000_000 },
        || client(&env, &contract).register_did(
            &controller,
            &text(&env, "did:stellar:alice"),
            &text(&env, "did:stellar:alice#key-1"),
            &Vec::new(&env),
        )
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=register_did metric=cpu_insns")]
fn a_registration_past_its_ceiling_fails_the_test_with_a_parseable_line() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);

    soroban_testkit_core::budget_guard!(&env, "register_did", { cpu_max: 1 }, || {
        client(&env, &contract).register_did(
            &controller,
            &text(&env, "did:stellar:alice"),
            &text(&env, "did:stellar:alice#key-1"),
            &Vec::new(&env),
        )
    });
}

#[test]
fn initialize_writes_only_instance_storage() {
    let (env, contract) = deployed();

    // The operator is configuration: it lives for as long as the instance does.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);

    let snapshot = StorageSnapshot::capture(&env, &contract);
    assert_eq!(snapshot.len(), 1);
    assert!(snapshot.in_tier(StorageTier::Persistent).is_empty());
}

#[test]
fn the_first_registration_opens_one_persistent_entry() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);
    let did = text(&env, "did:stellar:alice");

    let before = StorageSnapshot::capture(&env, &contract);
    client(&env, &contract).register_did(
        &controller,
        &did,
        &text(&env, "did:stellar:alice#key-1"),
        &Vec::new(&env),
    );
    let after = StorageSnapshot::capture(&env, &contract);

    // One document is one `(did, identifier)` key: the registry does not keep a
    // single map that every registration has to rewrite.
    let diff = before.diff(&after);
    assert_eq!(diff.added().len(), 1);
    assert_eq!(after.len(), 2);
    assert_eq!(
        after
            .in_tier(StorageTier::Persistent)
            .iter()
            .map(|entry| entry.tier)
            .collect::<StdVec<StorageTier>>(),
        [StorageTier::Persistent]
    );
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "[did*", StorageTier::Persistent);
}

#[test]
fn a_rotation_rewrites_the_document_and_adds_no_key() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);
    let next = Address::generate(&env);
    let did = text(&env, "did:stellar:alice");
    let client = client(&env, &contract);
    client.register_did(
        &controller,
        &did,
        &text(&env, "did:stellar:alice#key-1"),
        &Vec::new(&env),
    );

    let before = StorageSnapshot::capture(&env, &contract);
    client.rotate_controller(&did, &controller, &next);
    let after = StorageSnapshot::capture(&env, &contract);

    let diff = before.diff(&after);
    assert!(diff.added().is_empty(), "a rotation adds no key");
    assert!(diff.removed().is_empty(), "a rotation removes no key");
    assert_eq!(diff.modified().len(), 1);
    assert_eq!(diff.modified()[0].tier, StorageTier::Persistent);
    assert_eq!(after.len(), before.len());
}

#[test]
fn storage_footprint_grows_with_the_document_size() {
    // Document size assumptions, all measured in XDR bytes:
    //
    // - small:  the DID, the verification method id and version metadata, with
    //           no service endpoints.
    // - medium: two endpoints whose ids are `#svc-0`/`#svc-1` and whose URIs
    //           are 8 + 24 = 32 bytes.
    // - large:  eight endpoints (the cap), each URI 8 + 160 = 168 bytes.
    //
    // The numbers printed below are measured by this run, not estimated.
    let small = document_bytes(0, 0);
    let medium = document_bytes(2, 24);
    let large = document_bytes(8, 160);

    println!("services=0 uri_bytes=0   document_bytes={small}");
    println!("services=2 uri_bytes=32  document_bytes={medium}");
    println!("services=8 uri_bytes=168 document_bytes={large}");

    assert!(small > 0, "a document cannot occupy no bytes");
    assert!(
        medium > small,
        "two endpoints ({medium}) must occupy more than none ({small})"
    );
    assert!(
        large > medium,
        "eight long endpoints ({large}) must occupy more than two short ones ({medium})"
    );

    // Reproducible: a second measurement of the same document is the same
    // number, because it is XDR sizes rather than a heap reading.
    assert_eq!(document_bytes(2, 24), medium);
    assert_eq!(document_bytes(8, 160), large);
}

#[test]
fn a_credential_is_its_own_persistent_entry() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);
    let issuer = Address::generate(&env);
    let did = text(&env, "did:stellar:alice");
    let client = client(&env, &contract);
    client.register_did(
        &controller,
        &did,
        &text(&env, "did:stellar:alice#key-1"),
        &Vec::new(&env),
    );

    let before = StorageSnapshot::capture(&env, &contract);
    client.register_credential(&issuer, &text(&env, "vc:alice:degree:1"), &did);
    let after = StorageSnapshot::capture(&env, &contract);

    // A credential status is a key of its own: revoking one credential does not
    // rewrite the DID document or any other credential. The rendered key is
    // matched by prefix because the testkit renders a host `String` as its XDR
    // spelling; the leading `[cred` is what names the key space.
    let added = before.diff(&after);
    assert_eq!(added.added().len(), 1);
    assert!(added.added()[0].key.starts_with("[cred"));
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "[cred*", StorageTier::Persistent);

    let before_revocation = StorageSnapshot::capture(&env, &contract);
    client.revoke_credential(&issuer, &text(&env, "vc:alice:degree:1"));
    let after_revocation = StorageSnapshot::capture(&env, &contract);

    let diff = before_revocation.diff(&after_revocation);
    assert!(diff.added().is_empty() && diff.removed().is_empty());
    assert_eq!(diff.modified().len(), 1);
    assert!(diff.modified()[0].key.starts_with("[cred"));
}

#[test]
fn reading_state_leaves_storage_unchanged() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);
    let issuer = Address::generate(&env);
    let did = text(&env, "did:stellar:alice");
    let client = client(&env, &contract);
    client.register_did(
        &controller,
        &did,
        &text(&env, "did:stellar:alice#key-1"),
        &endpoints(&env, "did:stellar:alice", 1, 12),
    );
    client.register_credential(&issuer, &text(&env, "vc:alice:degree:1"), &did);

    let before = StorageSnapshot::capture(&env, &contract);
    client.resolve_did(&did);
    client.controller_of(&did);
    client.document_version(&did);
    client.service_endpoint(&did, &text(&env, "did:stellar:alice#svc-0"));
    client.credential(&text(&env, "vc:alice:degree:1"));
    client.is_revoked(&text(&env, "vc:alice:degree:1"));
    before.assert_unchanged(&StorageSnapshot::capture(&env, &contract));
}

#[test]
fn a_baseline_survives_the_file_ci_commits() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);
    let next = Address::generate(&env);
    let issuer = Address::generate(&env);
    let did = text(&env, "did:stellar:alice");
    let client = client(&env, &contract);

    let mut baseline = BudgetBaseline::new();
    client.register_did(
        &controller,
        &did,
        &text(&env, "did:stellar:alice#key-1"),
        &endpoints(&env, "did:stellar:alice", 2, 24),
    );
    baseline.record("register_did", BudgetSnapshot::last_invocation(&env));
    client.rotate_controller(&did, &controller, &next);
    baseline.record("rotate_controller", BudgetSnapshot::last_invocation(&env));
    client.register_credential(&issuer, &text(&env, "vc:alice:degree:1"), &did);
    baseline.record("register_credential", BudgetSnapshot::last_invocation(&env));
    client.revoke_credential(&issuer, &text(&env, "vc:alice:degree:1"));
    baseline.record("revoke_credential", BudgetSnapshot::last_invocation(&env));

    // A benchmark is a recorded measurement, not a threshold: the table is what
    // a reviewer compares a change against.
    for (case, cost) in baseline.cases() {
        println!(
            "{case:>20} cpu_insns={} mem_bytes={}",
            cost.cpu_insns, cost.mem_bytes
        );
    }

    // Round-tripping through the serialised form is what lets a pull request
    // compare its costs against a file recorded on the target branch.
    let reloaded = BudgetBaseline::parse(&baseline.to_json_string())
        .expect("a baseline this crate wrote parses");

    assert_eq!(reloaded.len(), 4);
    assert_eq!(
        reloaded
            .cases()
            .map(|(case, _)| case)
            .collect::<StdVec<&str>>(),
        [
            "register_credential",
            "register_did",
            "revoke_credential",
            "rotate_controller",
        ]
    );
    assert_eq!(reloaded.get("register_did"), baseline.get("register_did"));
    for case in ["register_did", "rotate_controller", "register_credential"] {
        let cost: BudgetSnapshot = baseline.get(case).unwrap();
        assert!(cost.cpu_insns > 0 && cost.mem_bytes > 0, "{case} metered");
    }
}

#[test]
#[should_panic(
    expected = "Storage tier mismatch for key 'admin': expected persistent, found instance"
)]
fn a_key_in_the_wrong_tier_is_a_failure() {
    let (env, contract) = deployed();

    // The operator is instance storage: a test that expects it to survive like a
    // persistent document is wrong about the contract and says so.
    soroban_testkit_core::assert_storage_tier!(&env, &contract, "admin", StorageTier::Persistent);
}

#[test]
fn the_document_key_is_the_only_rendered_shape_the_registry_writes() {
    let (env, contract) = deployed();
    let controller = Address::generate(&env);
    let did = text(&env, "did:stellar:alice");
    client(&env, &contract).register_did(
        &controller,
        &did,
        &text(&env, "did:stellar:alice#key-1"),
        &Vec::new(&env),
    );

    let snapshot = StorageSnapshot::capture(&env, &contract);
    let keys: StdVec<StdString> = snapshot
        .entries()
        .iter()
        .map(|entry| entry.qualified())
        .collect();

    // Two entries, and the document's key is the `did`-tagged tuple that carries
    // the identifier. The exact rendering of the value is the testkit's, so the
    // assertion is on the shape that names the key space.
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0], "instance:admin");
    assert!(keys[1].starts_with("persistent:[did, "));
    assert!(keys[1].contains("did:stellar:alice"));
}
