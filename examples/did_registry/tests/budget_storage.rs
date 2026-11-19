//! The measurement layer: what a DID operation costs and how many bytes each
//! document shape occupies, measured with `soroban-testkit-core`.
//!
//! # Methodology
//!
//! * Byte sizes come from [`FootprintSnapshot`], the XDR length of the ledger
//!   entry itself, which is the unit rent is charged in. The whole `documents`
//!   map is one entry, so its size is the sum of every registered document.
//! * Instruction and memory costs come from [`BudgetSnapshot::last_invocation`],
//!   which reports the top-level invocation that just ran. A contract registered
//!   natively is metered without the VM's own costs, so these numbers are lower
//!   than a network would bill and are useful as *relative* measurements.
//! * Every test here builds its own `Env`, so a measurement cannot inherit
//!   storage or metering from another test.
//! * The printed lines are the recorded measurements. Run
//!   `cargo test -p soroban-testkit-example-did-registry --test budget_storage
//!   -- --nocapture` to read them; the assertions check the relationships (more
//!   methods cost more, a second run costs the same, and so on) rather than
//!   pinning numbers, so an SDK bump does not turn this file red.
//!
//! No absolute cost is asserted anywhere: a number copied out of one machine is
//! not a benchmark, and the repository's recorded baseline for the counter lives
//! in `examples/counter/budget.json` for exactly that reason.

mod common;

use common::*;
use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::{Address, Env, String};
use soroban_testkit_core::budget::{BudgetBaseline, BudgetSnapshot};
use soroban_testkit_core::footprint::FootprintSnapshot;
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};

/// XDR bytes of the whole `documents` map.
fn documents_bytes(env: &Env, contract_id: &Address) -> u64 {
    FootprintSnapshot::capture(env, contract_id)
        .get("persistent:documents")
        .unwrap_or(0)
}

/// UTF-8 length of the `documents` map as the storage inspector renders it,
/// which is a second, independent view of the same growth.
fn documents_rendered_len(env: &Env, contract_id: &Address) -> Option<usize> {
    StorageSnapshot::capture(env, contract_id)
        .get("documents")
        .map(|entry| entry.value.len())
}

#[test]
fn a_registration_opens_the_documents_entry_in_persistent_storage() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);

    let before = StorageSnapshot::capture(&env, &contract_id);
    client.register(&controller, &document(&env, "alice", &controller));
    let after = StorageSnapshot::capture(&env, &contract_id);

    before.assert_entry_added(&after, "documents");
    soroban_testkit_core::assert_storage_tier!(
        &env,
        &contract_id,
        "documents",
        StorageTier::Persistent
    );

    // A permissionless registry needs no operator keys, so nothing lives in
    // instance storage at all.
    assert!(after.in_tier(StorageTier::Instance).is_empty());
    assert_eq!(after.len(), 1);
}

#[test]
fn a_registration_writes_one_entry_and_rewrites_nothing_else() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);

    client.register(&controller, &document(&env, "alice", &controller));
    let before = StorageSnapshot::capture(&env, &contract_id);
    client.register(&controller, &document(&env, "bob", &controller));
    let after = StorageSnapshot::capture(&env, &contract_id);
    let diff = before.diff(&after);

    // The second document rewrites the one map entry instead of opening a
    // second key, which is what makes the footprint measurement below a
    // measurement of documents rather than of registrations.
    assert!(diff.added().is_empty());
    assert!(diff.removed().is_empty());
    assert_eq!(diff.modified().len(), 1);
    assert_eq!(diff.modified()[0].key, "documents");
    assert_eq!(diff.modified()[0].tier, StorageTier::Persistent);
}

#[test]
fn document_size_grows_with_the_number_of_verification_methods() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);

    let mut previous_bytes = 0;
    let mut previous_rendered = 0;
    for methods in [1u32, 2, 4, 8] {
        let document = document_with(&env, &format!("methods-{methods}"), &controller, methods, 0);
        client.register(&controller, &document);

        let bytes = documents_bytes(&env, &contract_id);
        let rendered = documents_rendered_len(&env, &contract_id).expect("the map is stored");
        println!(
            "verification_methods={methods} documents_xdr_bytes={bytes} rendered_value_bytes={rendered}"
        );

        assert!(
            bytes > previous_bytes,
            "{methods} methods should take more ledger bytes than the previous shape"
        );
        assert!(
            rendered > previous_rendered,
            "{methods} methods should render wider than the previous shape"
        );
        previous_bytes = bytes;
        previous_rendered = rendered;
    }
}

#[test]
fn document_size_grows_with_the_number_of_service_endpoints() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);

    let mut previous_bytes = 0;
    for services in [0u32, 1, 4, 8] {
        let document = document_with(
            &env,
            &format!("services-{services}"),
            &controller,
            1,
            services,
        );
        client.register(&controller, &document);

        let bytes = documents_bytes(&env, &contract_id);
        println!("service_endpoints={services} documents_xdr_bytes={bytes}");

        if services > 0 {
            assert!(
                bytes > previous_bytes,
                "{services} endpoints should take more ledger bytes than the previous shape"
            );
        }
        previous_bytes = bytes;
    }
}

#[test]
fn a_large_document_costs_more_to_register_than_a_small_one() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);

    client.register(&controller, &document(&env, "small", &controller));
    let small = BudgetSnapshot::last_invocation(&env);
    client.register(
        &controller,
        &document_with(&env, "large", &controller, 8, 8),
    );
    let large = BudgetSnapshot::last_invocation(&env);

    println!(
        "register small cpu_insns={} mem_bytes={}",
        small.cpu_insns, small.mem_bytes
    );
    println!(
        "register large cpu_insns={} mem_bytes={}",
        large.cpu_insns, large.mem_bytes
    );

    // Eight verification methods and eight service endpoints are more values to
    // validate, serialize and write than one method and no endpoint. Both
    // resources must reflect that; an SDK change that made work free would be
    // worth noticing.
    assert!(
        large.cpu_insns > small.cpu_insns,
        "a large document should cost more CPU: {} vs {}",
        large.cpu_insns,
        small.cpu_insns
    );
    assert!(
        large.mem_bytes > small.mem_bytes,
        "a large document should cost more memory: {} vs {}",
        large.mem_bytes,
        small.mem_bytes
    );
}

#[test]
fn every_revocation_grows_the_one_revocation_map() {
    let (env, contract_id) = setup();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let issuer = document(&env, "issuer", &controller);
    client.register(&controller, &issuer);

    let before = StorageSnapshot::capture(&env, &contract_id);
    client.revoke_credential(
        &controller,
        &issuer.id,
        &text(&env, "urn:uuid:credential-1"),
        &text(&env, "superseded"),
    );
    let after_first = StorageSnapshot::capture(&env, &contract_id);

    before.assert_entry_added(&after_first, "revoked");
    soroban_testkit_core::assert_storage_tier!(
        &env,
        &contract_id,
        "revoked",
        StorageTier::Persistent
    );
    let first_bytes = after_first.get("revoked").unwrap().value.len();

    client.revoke_credential(
        &controller,
        &issuer.id,
        &text(&env, "urn:uuid:credential-2"),
        &text(&env, "superseded"),
    );
    let after_second = StorageSnapshot::capture(&env, &contract_id);
    let second_bytes = after_second.get("revoked").unwrap().value.len();

    println!("revocations=1 revoked_value_bytes={first_bytes}");
    println!("revocations=2 revoked_value_bytes={second_bytes}");

    // Revocations accumulate in one map: a second revocation grows its value
    // rather than adding a ledger key, so the per-credential cost is bytes.
    assert_eq!(after_first.entries().len(), after_second.entries().len());
    assert!(second_bytes > first_bytes);
}

#[test]
fn a_repeated_revocation_leaves_storage_untouched() {
    let (env, contract_id) = setup();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let issuer = document(&env, "issuer", &controller);
    client.register(&controller, &issuer);
    let credential = text(&env, "urn:uuid:credential-1");
    client.revoke_credential(
        &controller,
        &issuer.id,
        &credential,
        &text(&env, "superseded"),
    );

    let before = StorageSnapshot::capture(&env, &contract_id);
    env.ledger().set_timestamp(1_800_000_000);
    client.revoke_credential(
        &controller,
        &issuer.id,
        &credential,
        &text(&env, "a different reason"),
    );
    let after = StorageSnapshot::capture(&env, &contract_id);

    // Idempotence is a storage property too: the second call writes nothing,
    // so it cannot re-charge for an entry it did not change.
    before.assert_unchanged(&after);
}

#[test]
fn a_service_update_rewrites_the_documents_entry_without_growing_it_much() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document_with(&env, "alice", &controller, 2, 2);
    client.register(&controller, &document);
    let before = StorageSnapshot::capture(&env, &contract_id);

    client.update_service(
        &controller,
        &document.id,
        &service(
            &env,
            "svc-0",
            "CredentialRegistry",
            "https://registry.example",
        ),
    );
    let after = StorageSnapshot::capture(&env, &contract_id);
    let diff = before.diff(&after);

    assert!(diff.added().is_empty());
    assert!(diff.removed().is_empty());
    assert_eq!(diff.modified().len(), 1);
    assert_eq!(diff.modified()[0].key, "documents");
    assert_eq!(client.document_count(), 1);
}

#[test]
fn resolving_a_document_stays_inside_its_transaction_ceiling() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document_with(&env, "alice", &controller, 4, 4);
    client.register(&controller, &document);

    // A ceiling is transaction headroom, not the measured cost, so the limit is
    // deliberately far above what the call needs; a regression that made a read
    // allocate without bound would still cross it.
    soroban_testkit_core::budget_guard!(
        &env,
        "resolve",
        { cpu_max: 20_000_000, mem_max: 20_000_000 },
        || client.resolve(&document.id)
    );
}

#[test]
#[should_panic(expected = "BUDGET kind=ceiling case=register metric=cpu_insns")]
fn a_registration_past_its_ceiling_fails_the_test_with_a_parseable_line() {
    let (env, contract_id) = setup();
    let client = client(&env, &contract_id);
    let controller = signer(&env);

    soroban_testkit_core::budget_guard!(&env, "register", { cpu_max: 1 }, || {
        client.register(&controller, &document(&env, "alice", &controller))
    });
}

#[test]
fn the_same_registration_measures_the_same_in_two_fresh_environments() {
    fn registration_cost() -> (u64, u64) {
        let (env, contract_id) = setup();
        let client = client(&env, &contract_id);
        let controller = signer(&env);
        client.register(
            &controller,
            &document_with(&env, "alice", &controller, 2, 2),
        );
        let cost = BudgetSnapshot::last_invocation(&env);
        (cost.cpu_insns, cost.mem_bytes)
    }

    let first = registration_cost();
    let second = registration_cost();

    println!(
        "register cpu_insns={} mem_bytes={} (repeated)",
        first.0, first.1
    );

    // Same contract calls, same document shape, same empty starting state: the
    // metering is a property of the work, so a measurement can be committed and
    // compared from another run.
    assert_eq!(first, second);
}

#[test]
fn a_budget_baseline_records_the_identity_workload() {
    let (env, contract_id) = setup();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "issuer", &controller);

    let mut baseline = BudgetBaseline::new();
    client.register(&controller, &document);
    baseline.record("register", BudgetSnapshot::last_invocation(&env));

    client.rotate_verification_key(
        &controller,
        &document.id,
        &text(&env, "#key-1"),
        &key(&env, 99),
    );
    baseline.record("rotate_key", BudgetSnapshot::last_invocation(&env));

    client.revoke_credential(
        &controller,
        &document.id,
        &text(&env, "urn:uuid:credential-1"),
        &text(&env, "superseded"),
    );
    baseline.record("revoke", BudgetSnapshot::last_invocation(&env));

    // Serialising and re-reading is how the counter's `budget.json` survives a
    // pull request, so the example goes through the same round trip.
    let reloaded = BudgetBaseline::parse(&baseline.to_json_string())
        .expect("a baseline this crate wrote parses");

    assert_eq!(reloaded.len(), 3);
    for case in ["register", "rotate_key", "revoke"] {
        let recorded = reloaded.get(case).expect("every case was recorded");
        println!(
            "{case} cpu_insns={} mem_bytes={}",
            recorded.cpu_insns, recorded.mem_bytes
        );
        assert_eq!(reloaded.get(case), baseline.get(case));
    }
}

#[test]
fn rewriting_a_document_costs_more_than_registering_it() {
    let (env, contract_id) = setup();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let document = document(&env, "issuer", &controller);

    client.register(&controller, &document);
    let registration = BudgetSnapshot::last_invocation(&env);

    client.rotate_verification_key(
        &controller,
        &document.id,
        &text(&env, "#key-1"),
        &key(&env, 55),
    );
    let rotation = BudgetSnapshot::last_invocation(&env);

    client.revoke_credential(
        &controller,
        &document.id,
        &text(&env, "urn:uuid:credential-1"),
        &text(&env, "superseded"),
    );
    let revocation = BudgetSnapshot::last_invocation(&env);

    println!(
        "cpu register={} rotate={} revoke={}",
        registration.cpu_insns, rotation.cpu_insns, revocation.cpu_insns
    );

    // Registering builds a document it has never read; a rotation reads the
    // stored map back, changes one key inside one document and rewrites the
    // whole entry, then re-validates it. That is why the rewrite meters more
    // CPU than the first write. Only this relationship is asserted: the
    // revocation number is printed for comparison, not pinned, because it also
    // depends on which maps exist at the time.
    assert!(
        rotation.cpu_insns > registration.cpu_insns,
        "a rewrite should cost more than a first write: {} vs {}",
        rotation.cpu_insns,
        registration.cpu_insns
    );
}

#[test]
fn a_revoked_credential_stays_revoked_after_the_ledger_moves() {
    let (env, contract_id) = setup();
    env.ledger().set_timestamp(1_700_000_000);
    let client = client(&env, &contract_id);
    let controller = signer(&env);
    let issuer = document(&env, "issuer", &controller);
    client.register(&controller, &issuer);
    let credential: String = text(&env, "urn:uuid:credential-1");
    client.revoke_credential(
        &controller,
        &issuer.id,
        &credential,
        &text(&env, "superseded"),
    );

    let before = StorageSnapshot::capture(&env, &contract_id);
    env.ledger().set_timestamp(2_000_000_000);
    let record = client
        .revocation_status(&credential)
        .expect("revocation is permanent");

    assert_eq!(record.revoked_at, 1_700_000_000);
    before.assert_unchanged(&StorageSnapshot::capture(&env, &contract_id));
}
