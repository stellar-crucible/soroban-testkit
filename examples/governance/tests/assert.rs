//! Authorization, event and error assertions from `soroban-testkit-assert`,
//! focused on multi-user auth and timelock-bypass prevention.

mod common;

use common::{Fixture, TIMELOCK, VOTING_PERIOD};
use governance::{Action, GovernanceError, ProposalStatus};
use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
use soroban_sdk::IntoVal;
use soroban_testkit_assert::assert_contract_error;
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::EventMatcher;

#[test]
fn voting_requires_exactly_the_voters_auth() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(1, id, true);
    AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
    AuthMatcher::new(&fixture.ctx.env)
        .invoker(&fixture.voters[1])
        .function("vote")
        .assert_matches();
}

#[test]
fn each_member_authorizes_their_own_vote() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    for voter in 0..3 {
        fixture.vote(voter, id, true);
        AuthMatcher::new(&fixture.ctx.env)
            .invoker(&fixture.voters[voter])
            .function("vote")
            .assert_matches();
    }
}

#[test]
fn proposing_requires_the_proposers_auth() {
    let fixture = Fixture::funded();
    fixture.propose(&Action::SetTimelock(1));
    AuthMatcher::new(&fixture.ctx.env)
        .invoker(&fixture.voters[0])
        .function("propose")
        .assert_matches();
}

#[test]
fn minting_requires_the_admins_auth() {
    let fixture = Fixture::initialized();
    fixture.client().mint(&fixture.voters[0], &10);
    AuthMatcher::new(&fixture.ctx.env)
        .invoker(&fixture.admin)
        .function("mint")
        .assert_matches();
}

#[test]
fn queue_and_execute_are_permissionless() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(0, id, true);
    fixture.end_voting();
    fixture.client().queue(&id);
    AuthMatcher::new(&fixture.ctx.env).assert_no_auth_required();
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    AuthMatcher::new(&fixture.ctx.env).assert_no_auth_required();
}

#[test]
fn a_vote_signed_by_someone_else_is_refused() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let env = &fixture.ctx.env;
    // Only voter 2 signs, but the call claims to be voter 1's vote.
    env.mock_auths(&[MockAuth {
        address: &fixture.voters[2],
        invoke: &MockAuthInvoke {
            contract: &fixture.contract_id,
            fn_name: "vote",
            args: (fixture.voters[1].clone(), id, true).into_val(env),
            sub_invokes: &[],
        },
    }]);
    let result = fixture.client().try_vote(&fixture.voters[1], &id, &true);
    assert!(result.is_err());
    assert_eq!(fixture.client().proposal(&id).votes_for, 0);
}

#[test]
fn executing_a_pending_proposal_is_refused() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetQuorum(9_000));
    fixture.vote(0, id, true);
    fixture.vote(1, id, true);
    assert_contract_error!(
        fixture.client().try_execute(&id),
        GovernanceError::NotQueued
    );
}

#[test]
fn executing_before_the_timelock_is_refused() {
    let mut fixture = Fixture::funded();
    let id = fixture.queued(&Action::SetQuorum(9_000));
    fixture.advance_time(TIMELOCK - 1);
    assert_contract_error!(
        fixture.client().try_execute(&id),
        GovernanceError::TimelockNotElapsed
    );
    assert_eq!(fixture.client().quorum_bps(), common::QUORUM_BPS);
}

#[test]
fn execution_unlocks_exactly_at_the_eta() {
    let mut fixture = Fixture::funded();
    let id = fixture.queued(&Action::SetQuorum(9_000));
    let eta = fixture.client().proposal(&id).eta;
    assert_eq!(eta, fixture.now() + TIMELOCK);
    fixture.ctx.set_timestamp(eta);
    fixture.client().execute(&id);
    assert_eq!(fixture.status(id), ProposalStatus::Executed);
}

#[test]
fn a_proposal_cannot_be_executed_twice() {
    let mut fixture = Fixture::funded();
    let id = fixture.queued(&Action::Mint(fixture.outsider.clone(), 5));
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    assert_contract_error!(
        fixture.client().try_execute(&id),
        GovernanceError::NotQueued
    );
}

#[test]
fn a_defeated_proposal_can_never_execute() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetQuorum(9_000));
    fixture.vote(2, id, true);
    fixture.end_voting();
    fixture.client().queue(&id);
    fixture.advance_time(TIMELOCK * 10);
    assert_contract_error!(
        fixture.client().try_execute(&id),
        GovernanceError::NotQueued
    );
}

#[test]
fn a_queued_proposal_cannot_be_requeued_to_reset_its_eta() {
    let mut fixture = Fixture::funded();
    let id = fixture.queued(&Action::SetTimelock(0));
    fixture.advance_time(VOTING_PERIOD);
    assert_contract_error!(fixture.client().try_queue(&id), GovernanceError::NotPending);
}

#[test]
fn each_state_change_emits_one_event() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("proposal_created")
        .assert_count(1);
    fixture.vote(0, id, true);
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("vote_cast")
        .assert_count(1);
    fixture.end_voting();
    fixture.client().queue(&id);
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("proposal_queued")
        .assert_count(1);
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("proposal_executed")
        .assert_count(1);
}

#[test]
fn a_defeat_emits_a_defeat_event() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.end_voting();
    fixture.client().queue(&id);
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("proposal_defeated")
        .assert_count(1);
    EventMatcher::new(&fixture.ctx.env)
        .with_topic("proposal_queued")
        .assert_not_emitted();
}
