mod common;

use common::{Fixture, TIMELOCK};
use multisig_vault::VaultError;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::EventMatcher;

#[test]
fn deposit_emits_event() {
    let fixture = Fixture::initialized();
    fixture.fund(1_000);
    EventMatcher::new(&fixture.ctx.env).assert_count(1);
}

#[test]
fn propose_emits_event() {
    let fixture = Fixture::funded();
    fixture.propose(250);
    EventMatcher::new(&fixture.ctx.env).assert_count(1);
}

#[test]
fn approve_emits_event() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.propose(250);
    fixture.client().approve(&fixture.signers[0], &proposal_id);
    EventMatcher::new(&fixture.ctx.env).assert_count(1);
}

#[test]
fn execute_emits_event() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(250);
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&fixture.signers[0], &proposal_id);
    EventMatcher::new(&fixture.ctx.env).assert_count(1);
}

#[test]
fn cancel_emits_event() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(250);
    fixture.client().cancel(&fixture.signers[2], &proposal_id);
    EventMatcher::new(&fixture.ctx.env).assert_count(1);
}

#[test]
fn deposit_requires_exactly_one_auth() {
    let fixture = Fixture::initialized();
    fixture.fund(1_000);
    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
}

#[test]
fn propose_requires_exactly_one_auth() {
    let fixture = Fixture::funded();
    fixture.propose(100);
    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
}

#[test]
fn execute_before_timelock_fails() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(250);
    fixture.advance_time(TIMELOCK - 1);
    let result = fixture
        .client()
        .try_execute(&fixture.signers[0], &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::TimelockNotElapsed);
}

#[test]
fn execute_without_threshold_fails() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.propose(250);
    fixture.advance_time(TIMELOCK);
    let result = fixture
        .client()
        .try_execute(&fixture.signers[0], &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::ThresholdNotMet);
}

#[test]
fn execute_after_timelock_reduces_balance() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(2_500);
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&fixture.signers[0], &proposal_id);
    assert_eq!(fixture.client().balance(), common::INITIAL_BALANCE - 2_500);
}

#[test]
fn execute_marks_proposal_executed() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(2_500);
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&fixture.signers[0], &proposal_id);
    assert!(fixture.client().proposal(&proposal_id).unwrap().executed);
}

#[test]
fn cancel_during_timelock_prevents_execution() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(2_500);
    let client = fixture.client();
    client.cancel(&fixture.signers[2], &proposal_id);

    fixture.advance_time(TIMELOCK);
    let result = client.try_execute(&fixture.signers[0], &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::AlreadyCancelled);
    assert_eq!(client.balance(), common::INITIAL_BALANCE);
}

#[test]
fn cancel_after_ready_still_works() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(2_500);
    fixture.advance_time(TIMELOCK);
    fixture.client().cancel(&fixture.signers[1], &proposal_id);
    assert!(fixture.client().proposal(&proposal_id).unwrap().cancelled);
}

#[test]
fn non_signer_cannot_execute() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(250);
    fixture.advance_time(TIMELOCK);
    let result = fixture
        .client()
        .try_execute(&fixture.outsider, &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::Unauthorized);
}

#[test]
fn non_signer_cannot_cancel() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(250);
    let result = fixture.client().try_cancel(&fixture.outsider, &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::Unauthorized);
}

#[test]
fn execute_cannot_run_twice() {
    let fixture = Fixture::funded();
    let proposal_id = fixture.ready_proposal(250);
    fixture.advance_time(TIMELOCK);
    let client = fixture.client();
    client.execute(&fixture.signers[0], &proposal_id);
    let result = client.try_execute(&fixture.signers[0], &proposal_id);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::AlreadyExecuted);
}

#[test]
fn execute_unknown_proposal_fails() {
    let fixture = Fixture::funded();
    let result = fixture.client().try_execute(&fixture.signers[0], &99);
    assert_eq!(result.unwrap_err().unwrap(), VaultError::ProposalNotFound);
}
