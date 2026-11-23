mod common;

use common::{Fixture, THRESHOLD, TIMELOCK};
use guardian_treasury::TreasuryError;
use soroban_sdk::Vec;

#[test]
fn initialize_records_guardian_and_threshold() {
    let fixture = Fixture::initialized();
    assert_eq!(fixture.client().guardian(), Some(fixture.guardian.clone()));
    assert_eq!(fixture.client().threshold(), THRESHOLD);
}

#[test]
fn initialize_records_signers_and_timelock() {
    let fixture = Fixture::initialized();
    assert_eq!(fixture.client().signers().len(), 3);
    assert_eq!(fixture.client().timelock(), TIMELOCK);
}

#[test]
fn initialize_starts_with_zero_balance() {
    let fixture = Fixture::initialized();
    assert_eq!(fixture.client().balance(), 0);
}

#[test]
fn initialize_rejects_empty_signers() {
    let fixture = Fixture::new();
    let result = fixture.client().try_initialize(
        &fixture.admin,
        &Vec::new(&fixture.ctx.env),
        &1,
        &TIMELOCK,
        &fixture.guardian,
    );
    assert_eq!(result.unwrap_err().unwrap(), TreasuryError::EmptySigners);
}

#[test]
fn initialize_rejects_invalid_thresholds() {
    let fixture = Fixture::new();
    let client = fixture.client();
    let signers = fixture.signers_vec();
    assert_eq!(
        client
            .try_initialize(&fixture.admin, &signers, &0, &TIMELOCK, &fixture.guardian)
            .unwrap_err()
            .unwrap(),
        TreasuryError::InvalidThreshold
    );
    assert_eq!(
        client
            .try_initialize(&fixture.admin, &signers, &4, &TIMELOCK, &fixture.guardian)
            .unwrap_err()
            .unwrap(),
        TreasuryError::InvalidThreshold
    );
}

#[test]
fn initialize_rejects_zero_timelock() {
    let fixture = Fixture::new();
    let result = fixture.client().try_initialize(
        &fixture.admin,
        &fixture.signers_vec(),
        &THRESHOLD,
        &0,
        &fixture.guardian,
    );
    assert_eq!(result.unwrap_err().unwrap(), TreasuryError::InvalidTimelock);
}

#[test]
fn initialize_rejects_duplicate_signers() {
    let fixture = Fixture::new();
    let duplicates = Vec::from_array(
        &fixture.ctx.env,
        [fixture.signers[0].clone(), fixture.signers[0].clone()],
    );
    let result = fixture.client().try_initialize(
        &fixture.admin,
        &duplicates,
        &1,
        &TIMELOCK,
        &fixture.guardian,
    );
    assert_eq!(result.unwrap_err().unwrap(), TreasuryError::DuplicateSigner);
}

#[test]
fn initialized_vault_cannot_be_initialized_twice() {
    let fixture = Fixture::initialized();
    let result = fixture.client().try_initialize(
        &fixture.admin,
        &fixture.signers_vec(),
        &THRESHOLD,
        &TIMELOCK,
        &fixture.guardian,
    );
    assert_eq!(
        result.unwrap_err().unwrap(),
        TreasuryError::AlreadyInitialized
    );
}

#[test]
fn deposits_accumulate_with_checked_positive_amounts() {
    let fixture = Fixture::initialized();
    fixture.fund(900);
    fixture.fund(100);
    assert_eq!(fixture.client().balance(), 1_000);
    assert_eq!(
        fixture
            .client()
            .try_deposit(&fixture.admin, &-1)
            .unwrap_err()
            .unwrap(),
        TreasuryError::InvalidAmount
    );
}

#[test]
fn proposal_is_persisted_with_pending_status() {
    let fixture = Fixture::funded();
    let id = fixture.propose(500);
    let proposal = fixture.client().proposal(&id).unwrap();
    assert_eq!(proposal.id, id);
    assert_eq!(proposal.amount, 500);
    assert_eq!(proposal.approvals.len(), 0);
    assert!(!proposal.vetoed);
    assert!(!proposal.executed);
}

#[test]
fn proposal_ids_increase_monotonically() {
    let fixture = Fixture::funded();
    assert_eq!(fixture.propose(100), 0);
    assert_eq!(fixture.propose(200), 1);
    assert_eq!(fixture.client().next_proposal_id(), 2);
}

#[test]
fn non_signer_cannot_propose() {
    let fixture = Fixture::funded();
    let result = fixture
        .client()
        .try_propose(&fixture.outsider, &fixture.recipient, &10);
    assert_eq!(result.unwrap_err().unwrap(), TreasuryError::Unauthorized);
}

#[test]
fn proposal_cannot_exceed_treasury_balance() {
    let fixture = Fixture::funded();
    let result = fixture.client().try_propose(
        &fixture.signers[0],
        &fixture.recipient,
        &(common::INITIAL_BALANCE + 1),
    );
    assert_eq!(
        result.unwrap_err().unwrap(),
        TreasuryError::InsufficientBalance
    );
}
