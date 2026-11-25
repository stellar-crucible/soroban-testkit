mod common;

use common::{Fixture, INITIAL_BALANCE, TIMELOCK};
use guardian_treasury::TreasuryError;
use soroban_testkit_assert::assert_contract_error;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::EventMatcher;

#[test]
fn proposal_requires_signer_auth() {
    let fixture = Fixture::funded();
    fixture.propose(100);
    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
}

#[test]
fn guardian_veto_emits_event_and_requires_guardian_auth() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(1_000);
    fixture.client().veto(&fixture.guardian, &id);
    EventMatcher::new(&fixture.ctx.env).assert_count(1);
    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
}

#[test]
fn non_guardian_cannot_veto() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(100);
    assert_contract_error!(
        fixture.client().try_veto(&fixture.outsider, &id),
        TreasuryError::GuardianUnauthorized
    );
}

#[test]
fn guardian_can_veto_approved_proposal_during_delay() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(2_000);
    fixture.advance_time(TIMELOCK - 1);
    fixture.client().veto(&fixture.guardian, &id);
    assert!(fixture.client().proposal(&id).unwrap().vetoed);
    assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
}

#[test]
fn veto_before_threshold_is_rejected() {
    let fixture = Fixture::funded();
    let id = fixture.propose(1_000);
    assert_contract_error!(
        fixture.client().try_veto(&fixture.guardian, &id),
        TreasuryError::VetoWindowClosed
    );
}

#[test]
fn veto_after_timelock_window_is_rejected() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(1_000);
    fixture.advance_time(TIMELOCK);
    assert_contract_error!(
        fixture.client().try_veto(&fixture.guardian, &id),
        TreasuryError::VetoWindowClosed
    );
}

#[test]
fn vetoed_proposal_cannot_be_executed() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(2_000);
    fixture.client().veto(&fixture.guardian, &id);
    fixture.advance_time(TIMELOCK);
    assert_contract_error!(
        fixture.client().try_execute(&fixture.signers[0], &id),
        TreasuryError::AlreadyVetoed
    );
    assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
}

#[test]
fn threshold_and_timelock_allow_execution() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(2_500);
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&fixture.signers[0], &id);
    assert!(fixture.client().proposal(&id).unwrap().executed);
    assert_eq!(fixture.client().balance(), INITIAL_BALANCE - 2_500);
}

#[test]
fn execution_before_delay_is_rejected() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(100);
    fixture.advance_time(TIMELOCK - 1);
    assert_contract_error!(
        fixture.client().try_execute(&fixture.signers[0], &id),
        TreasuryError::TimelockNotElapsed
    );
}

#[test]
fn threshold_is_required_to_execute() {
    let fixture = Fixture::funded();
    let id = fixture.propose(100);
    fixture.advance_time(TIMELOCK);
    assert_contract_error!(
        fixture.client().try_execute(&fixture.signers[0], &id),
        TreasuryError::ThresholdNotMet
    );
}

#[test]
fn duplicate_approval_does_not_inflate_threshold() {
    let fixture = Fixture::funded();
    let id = fixture.propose(100);
    let client = fixture.client();
    client.approve(&fixture.signers[0], &id);
    assert_contract_error!(
        client.try_approve(&fixture.signers[0], &id),
        TreasuryError::AlreadyApproved
    );
    assert_eq!(client.proposal(&id).unwrap().approvals.len(), 1);
}

#[test]
fn guardian_cannot_veto_twice() {
    let fixture = Fixture::funded();
    let id = fixture.ready_proposal(100);
    let client = fixture.client();
    client.veto(&fixture.guardian, &id);
    assert_contract_error!(
        client.try_veto(&fixture.guardian, &id),
        TreasuryError::AlreadyVetoed
    );
}
