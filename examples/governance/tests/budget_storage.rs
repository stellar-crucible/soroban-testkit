//! Budget and storage inspection with `soroban-testkit-core`: which tier each
//! key lives in, and how a proposal's entry changes across its life cycle.

mod common;

use common::{Fixture, TIMELOCK};
use governance::Action;
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::storage::{assert_storage_tier, StorageSnapshot, StorageTier};
use soroban_testkit_fixtures::profiles::{TESTNET_CPU_INSTRUCTIONS, TESTNET_MEM_BYTES};

fn snapshot(fixture: &Fixture) -> StorageSnapshot {
    StorageSnapshot::capture(&fixture.ctx.env, &fixture.contract_id)
}

fn budget(fixture: &Fixture) -> BudgetSnapshot {
    BudgetSnapshot::capture(&fixture.ctx.env.host().budget_cloned())
}

#[test]
fn configuration_lives_in_instance_storage() {
    let fixture = Fixture::initialized();
    let env = &fixture.ctx.env;
    for key in [
        "[Admin]",
        "[QuorumBps]",
        "[VotingPeriod]",
        "[Timelock]",
        "[TotalSupply]",
        "[NextProposalId]",
    ] {
        assert_storage_tier(env, &fixture.contract_id, key, StorageTier::Instance);
    }
}

#[test]
fn power_proposals_and_receipts_live_in_persistent_storage() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(0, id, true);
    let env = &fixture.ctx.env;
    assert_storage_tier(
        env,
        &fixture.contract_id,
        "[Power*",
        StorageTier::Persistent,
    );
    assert_storage_tier(
        env,
        &fixture.contract_id,
        "[Proposal*",
        StorageTier::Persistent,
    );
    assert_storage_tier(
        env,
        &fixture.contract_id,
        "[Receipt*",
        StorageTier::Persistent,
    );
    assert!(snapshot(&fixture)
        .in_tier(StorageTier::Temporary)
        .is_empty());
}

#[test]
fn a_vote_adds_one_receipt_and_rewrites_the_proposal() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let before = snapshot(&fixture);
    fixture.vote(1, id, false);
    let diff = before.diff(&snapshot(&fixture));

    assert_eq!(diff.added().len(), 1, "{diff:?}");
    assert!(diff.added()[0].key.starts_with("[Receipt, 0,"));
    assert_eq!(diff.added()[0].tier, StorageTier::Persistent);
    assert_eq!(diff.modified().len(), 1, "{diff:?}");
    assert_eq!(diff.modified()[0].key, "[Proposal, 0]");
    assert!(diff.removed().is_empty());
}

#[test]
fn the_proposal_entry_moves_pending_queued_executed() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetQuorum(5_000));
    fixture.vote(0, id, true);
    fixture.vote(1, id, true);
    fixture.end_voting();

    let pending = snapshot(&fixture);
    assert!(pending
        .get("[Proposal, 0]")
        .unwrap()
        .value
        .contains("Pending"));

    fixture.client().queue(&id);
    let queued = snapshot(&fixture);
    let change = pending.diff(&queued);
    let proposal = change
        .modified()
        .iter()
        .find(|entry| entry.key == "[Proposal, 0]")
        .expect("queue rewrites the proposal");
    assert!(proposal.before.contains("Pending"));
    assert!(proposal.after.contains("Queued"));

    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    let executed = snapshot(&fixture);
    let change = queued.diff(&executed);
    let keys: Vec<_> = change
        .modified()
        .iter()
        .map(|entry| entry.key.as_str())
        .collect();
    assert!(keys.contains(&"[Proposal, 0]"));
    assert!(
        keys.contains(&"[QuorumBps]"),
        "execution applies the action: {keys:?}"
    );
    let proposal = change
        .modified()
        .iter()
        .find(|entry| entry.key == "[Proposal, 0]")
        .unwrap();
    assert!(proposal.before.contains("Queued"));
    assert!(proposal.after.contains("Executed"));
}

#[test]
fn a_rejected_execution_leaves_storage_untouched() {
    let mut fixture = Fixture::funded();
    let id = fixture.queued(&Action::SetQuorum(5_000));
    let before = snapshot(&fixture);
    let _ = fixture.client().try_execute(&id);
    before.assert_unchanged(&snapshot(&fixture));
}

#[test]
fn persistent_entries_carry_a_ttl() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let snapshot = snapshot(&fixture);
    let entry = snapshot.get(&format!("[Proposal, {id}]")).unwrap();
    let live_until = entry.live_until.expect("persistent entries expire");
    assert!(live_until > fixture.ctx.sequence());
}

#[test]
fn a_vote_consumes_budget() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let before = budget(&fixture);
    fixture.vote(0, id, true);
    let diff = before.diff(&budget(&fixture));
    assert!(diff.cpu_insns > 0 || diff.mem_bytes > 0, "{diff:?}");
}

#[test]
fn every_entry_point_fits_the_testnet_budget() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let measure = |fixture: &Fixture, call: &dyn Fn(&Fixture)| {
        fixture.ctx.env.cost_estimate().budget().reset_default();
        call(fixture);
        budget(fixture)
    };
    let vote = measure(&fixture, &|f| {
        f.vote(0, id, true);
    });
    assert!(vote.cpu_insns < TESTNET_CPU_INSTRUCTIONS as u64, "{vote:?}");
    assert!(vote.mem_bytes < TESTNET_MEM_BYTES as u64, "{vote:?}");
    fixture.vote(1, id, true);
    fixture.end_voting();
    let queue = measure(&fixture, &|f| {
        f.client().queue(&id);
    });
    assert!(
        queue.cpu_insns < TESTNET_CPU_INSTRUCTIONS as u64,
        "{queue:?}"
    );
}
