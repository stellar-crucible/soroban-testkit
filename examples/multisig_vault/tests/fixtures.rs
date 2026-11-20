mod common;

use common::{Fixture, INITIAL_BALANCE, THRESHOLD, TIMELOCK};
use multisig_vault::VaultError;
use soroban_sdk::Vec;

#[test]
fn initialize_sets_threshold() {
    let fixture = Fixture::initialized();
    assert_eq!(fixture.client().threshold(), THRESHOLD);
}

#[test]
fn initialize_sets_timelock() {
    let fixture = Fixture::initialized();
    assert_eq!(fixture.client().timelock(), TIMELOCK);
}

#[test]
fn initialize_records_signers() {
    let fixture = Fixture::initialized();
    let signers = fixture.client().signers();
    assert_eq!(signers.len(), 3);
    for signer in &fixture.signers {
        assert!(fixture.client().is_signer(signer));
    }
}

#[test]
fn initialize_starts_with_zero_balance() {
    let fixture = Fixture::initialized();
    assert_eq!(fixture.client().balance(), 0);
}

#[test]
fn initialize_rejects_empty_signers() {
    let fixture = Fixture::new();
    let empty = Vec::new(&fixture.ctx.env);
    let result = fixture
        .client()
        .try_initialize(&fixture.admin, &empty, &1, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::EmptySigners);
}

#[test]
fn initialize_rejects_zero_threshold() {
    let fixture = Fixture::new();
    let result =
        fixture
            .client()
            .try_initialize(&fixture.admin, &fixture.signers_vec(), &0, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::InvalidThreshold);
}

#[test]
fn initialize_rejects_threshold_above_signer_count() {
    let fixture = Fixture::new();
    let result =
        fixture
            .client()
            .try_initialize(&fixture.admin, &fixture.signers_vec(), &4, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::InvalidThreshold);
}

#[test]
fn initialize_rejects_duplicate_signers() {
    let fixture = Fixture::new();
    let duplicate = Vec::from_array(
        &fixture.ctx.env,
        [fixture.signers[0].clone(), fixture.signers[0].clone()],
    );
    let result = fixture
        .client()
        .try_initialize(&fixture.admin, &duplicate, &1, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::DuplicateSigner);
}

#[test]
fn initialize_rejects_zero_timelock() {
    let fixture = Fixture::new();
    let result =
        fixture
            .client()
            .try_initialize(&fixture.admin, &fixture.signers_vec(), &THRESHOLD, &0);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::InvalidTimelock);
}

#[test]
fn initialize_cannot_run_twice() {
    let fixture = Fixture::initialized();
    let result = fixture.client().try_initialize(
        &fixture.admin,
        &fixture.signers_vec(),
        &THRESHOLD,
        &TIMELOCK,
    );
    assert_eq!(result.unwrap_err().unwrap(), VaultError::AlreadyInitialized);
}

#[test]
fn deposit_increases_balance() {
    let fixture = Fixture::initialized();
    fixture.fund(2_500);
    assert_eq!(fixture.client().balance(), 2_500);
    fixture.fund(500);
    assert_eq!(fixture.client().balance(), 3_000);
}

#[test]
fn deposit_rejects_zero_amount() {
    let fixture = Fixture::initialized();
    let result = fixture.client().try_deposit(&fixture.admin, &0);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::InvalidAmount);
}

#[test]
fn deposit_rejects_negative_amount() {
    let fixture = Fixture::initialized();
    let result = fixture.client().try_deposit(&fixture.admin, &-1);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::InvalidAmount);
}

#[test]
fn propose_creates_pending_proposal() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.propose(2_500);
    let proposal = fixture.client().proposal(&proposal_id).unwrap();

    assert_eq!(proposal.id, proposal_id);
    assert_eq!(proposal.proposer, fixture.signers[0]);
    assert_eq!(proposal.recipient, fixture.recipient);
    assert_eq!(proposal.amount, 2_500);
    assert_eq!(proposal.approvals.len(), 0);
    assert_eq!(proposal.ready_at, 0);
    assert!(!proposal.executed);
    assert!(!proposal.cancelled);
}

#[test]
fn propose_assigns_incrementing_ids() {
    let fixture = Fixture::funded();
    assert_eq!(fixture.propose(100), 0);
    assert_eq!(fixture.propose(200), 1);
    assert_eq!(fixture.propose(300), 2);
    assert_eq!(fixture.client().next_proposal_id(), 3);
}

#[test]
fn propose_rejects_non_signer() {
    let fixture = Fixture::funded();
    let result =
        fixture
            .client()
            .try_propose_withdrawal(&fixture.outsider, &fixture.recipient, &100);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::Unauthorized);
}

#[test]
fn propose_rejects_amount_above_balance() {
    let fixture = Fixture::funded();
    let result = fixture.client().try_propose_withdrawal(
        &fixture.signers[0],
        &fixture.recipient,
        &(INITIAL_BALANCE + 1),
    );
    assert_eq!(
        result.unwrap_err().unwrap(),
        VaultError::InsufficientBalance
    );
}

#[test]
fn approve_records_approval() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.propose(500);
    fixture.client().approve(&fixture.signers[0], &proposal_id);

    let proposal = fixture.client().proposal(&proposal_id).unwrap();
    assert_eq!(proposal.approvals.len(), 1);
    assert_eq!(proposal.ready_at, 0);
}

#[test]
fn approve_rejects_duplicate_approval() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.propose(500);
    let client = fixture.client();
    client.approve(&fixture.signers[0], &proposal_id);
    let result = client.try_approve(&fixture.signers[0], &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::AlreadyApproved);
}

#[test]
fn approve_rejects_non_signer() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.propose(500);
    let result = fixture
        .client()
        .try_approve(&fixture.outsider, &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::Unauthorized);
}

#[test]
fn approve_sets_ready_at_when_threshold_met() {
    let fixture = Fixture::funded();
    let start = fixture.now();
    let proposal_id = fixture.propose(500);

    let client = fixture.client();
    client.approve(&fixture.signers[0], &proposal_id);
    assert_eq!(
        client.proposal(&proposal_id).unwrap().ready_at,
        0,
        "not queued before the threshold is reached"
    );

    client.approve(&fixture.signers[1], &proposal_id);
    let proposal = client.proposal(&proposal_id).unwrap();
    assert_eq!(proposal.ready_at, start + TIMELOCK);
}
