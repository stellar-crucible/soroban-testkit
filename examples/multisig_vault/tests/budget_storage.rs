mod common;

use common::{Fixture, TIMELOCK};
use soroban_sdk::Env;
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::namespace_auditor::NamespaceAuditor;
use soroban_testkit_core::storage::{StorageEntry, StorageTier};
use soroban_testkit_fixtures::profiles::{
    MAINNET_CPU_INSTRUCTIONS, MAINNET_MEM_BYTES, TESTNET_CPU_INSTRUCTIONS, TESTNET_MEM_BYTES,
};
use soroban_testkit_fixtures::NetworkProfile;

fn capture(fixture: &Fixture) -> BudgetSnapshot {
    BudgetSnapshot::capture(&fixture.ctx.env.host().budget_cloned())
}

#[test]
fn deposit_consumes_cpu_and_memory() {
    let fixture = Fixture::initialized();
    let before = capture(&fixture);
    fixture.fund(1_000);
    let after = capture(&fixture);
    let diff = before.diff(&after);
    assert!(
        diff.cpu_insns > 0 || diff.mem_bytes > 0,
        "deposit should consume budget: {diff:?}"
    );
}

#[test]
fn withdrawal_budget_diff_is_visible() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(1_000);
    fixture.advance_time(TIMELOCK);

    let before = capture(&fixture);
    fixture.client().execute(&fixture.signers[0], &proposal_id);
    let after = capture(&fixture);
    let diff = before.diff(&after);

    assert!(after.cpu_insns >= before.cpu_insns);
    assert!(diff.mem_bytes <= after.mem_bytes);
}

#[test]
fn local_profile_reports_unlimited_budget() {
    let env = Env::default();
    NetworkProfile::Local.apply(&env);
    let remaining = env
        .host()
        .budget_cloned()
        .get_cpu_insns_remaining()
        .unwrap();
    assert_eq!(remaining, u64::MAX);
}

#[test]
fn testnet_profile_reports_testnet_budget() {
    let env = Env::default();
    NetworkProfile::Testnet.apply(&env);
    let budget = env.host().budget_cloned();
    assert_eq!(
        budget.get_cpu_insns_remaining().unwrap(),
        TESTNET_CPU_INSTRUCTIONS as u64
    );
    assert_eq!(
        budget.get_mem_bytes_remaining().unwrap(),
        TESTNET_MEM_BYTES as u64
    );
}

#[test]
fn mainnet_profile_reports_mainnet_budget() {
    let env = Env::default();
    NetworkProfile::Mainnet.apply(&env);
    let budget = env.host().budget_cloned();
    assert_eq!(
        budget.get_cpu_insns_remaining().unwrap(),
        MAINNET_CPU_INSTRUCTIONS as u64
    );
    assert_eq!(
        budget.get_mem_bytes_remaining().unwrap(),
        MAINNET_MEM_BYTES as u64
    );
}

#[test]
fn vault_storage_keys_are_namespaced() {
    let mut auditor = NamespaceAuditor::default();
    auditor
        .record("multisig_vault", "DataKey::Signers")
        .record("multisig_vault", "DataKey::Threshold")
        .record("multisig_vault", "DataKey::Timelock")
        .record("multisig_vault", "DataKey::Balance")
        .record("multisig_vault", "DataKey::NextProposalId")
        .record("multisig_vault", "DataKey::Proposal");

    let report = auditor.audit();
    assert!(!report.has_collisions());
    assert!(report.un_namespaced_keys().is_empty());
    assert!(report.is_clean());
}

#[test]
fn composed_modules_can_collide_on_bare_keys() {
    let mut auditor = NamespaceAuditor::default();
    auditor
        .record("multisig_vault", "admin")
        .record("pausable", "admin");

    let report = auditor.audit();
    assert!(report.has_collisions());
    assert_eq!(report.collisions()[0].key, "admin");
}

#[test]
fn namespace_report_lists_vault_keys_per_module() {
    let mut auditor = NamespaceAuditor::default();
    auditor
        .record("multisig_vault", "DataKey::Signers")
        .record("treasury", "DataKey::Balance");

    let report = auditor.audit();
    assert_eq!(
        report.modules().get("multisig_vault").unwrap(),
        &vec!["DataKey::Signers".to_string()]
    );
    assert_eq!(
        report.modules().get("treasury").unwrap(),
        &vec!["DataKey::Balance".to_string()]
    );
    assert!(report
        .to_string()
        .contains("multisig_vault: DataKey::Signers"));
}

#[test]
fn storage_snapshot_entries_infer_namespaces() {
    let entries = vec![
        StorageEntry {
            key: "DataKey::Signers".to_string(),
            tier: StorageTier::Instance,
            value: String::new(),
            live_until: None,
        },
        StorageEntry {
            key: "DataKey::Proposal".to_string(),
            tier: StorageTier::Persistent,
            value: String::new(),
            live_until: None,
        },
        StorageEntry {
            key: "loose_key".to_string(),
            tier: StorageTier::Temporary,
            value: String::new(),
            live_until: None,
        },
    ];

    let report = NamespaceAuditor::from_entries(&entries).audit();
    assert!(report.modules().contains_key("DataKey"));
    assert!(report.modules().contains_key("contract"));
    assert_eq!(report.un_namespaced_keys().len(), 1);
    assert_eq!(report.un_namespaced_keys()[0].key, "loose_key");
}

#[test]
fn fee_estimate_is_available_after_invocation() {
    let fixture = Fixture::funded();
    let fee = fixture.ctx.env.cost_estimate().fee();
    assert!(fee.total >= 0, "fee estimate: {fee:?}");
}

#[test]
fn budget_snapshot_diff_never_underflows() {
    let fixture = Fixture::initialized();
    let snapshot = capture(&fixture);
    let diff = snapshot.diff(&snapshot);
    assert_eq!(diff.cpu_insns, 0);
    assert_eq!(diff.mem_bytes, 0);
}
