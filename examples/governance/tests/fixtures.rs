//! Life-cycle tests built on `soroban-testkit-fixtures`: a `TestContext` with
//! generated members, a controllable ledger clock and network profiles.

mod common;

use common::{Fixture, QUORUM_BPS, SUPPLY, TIMELOCK, VOTING_PERIOD, WEIGHTS};
use governance::{Action, GovernanceError, ProposalStatus};
use soroban_testkit_fixtures::NetworkProfile;

#[test]
fn initialize_stores_the_configuration() {
    let fixture = Fixture::initialized();
    let client = fixture.client();
    assert_eq!(client.admin(), fixture.admin);
    assert_eq!(client.quorum_bps(), QUORUM_BPS);
    assert_eq!(client.voting_period(), VOTING_PERIOD);
    assert_eq!(client.timelock(), TIMELOCK);
    assert_eq!(client.total_supply(), 0);
    assert_eq!(client.proposal_count(), 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let fixture = Fixture::initialized();
    let result =
        fixture
            .client()
            .try_initialize(&fixture.admin, &QUORUM_BPS, &VOTING_PERIOD, &TIMELOCK);
    assert_eq!(
        result.unwrap_err().unwrap(),
        GovernanceError::AlreadyInitialized
    );
}

#[test]
fn initialize_rejects_a_zero_quorum() {
    let fixture = Fixture::new();
    let result = fixture
        .client()
        .try_initialize(&fixture.admin, &0, &VOTING_PERIOD, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::InvalidQuorum);
}

#[test]
fn initialize_rejects_a_quorum_above_one_hundred_percent() {
    let fixture = Fixture::new();
    let result =
        fixture
            .client()
            .try_initialize(&fixture.admin, &10_001, &VOTING_PERIOD, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::InvalidQuorum);
}

#[test]
fn initialize_rejects_a_zero_voting_period() {
    let fixture = Fixture::new();
    let result = fixture
        .client()
        .try_initialize(&fixture.admin, &QUORUM_BPS, &0, &TIMELOCK);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::InvalidPeriod);
}

#[test]
fn minting_accumulates_power_and_supply() {
    let fixture = Fixture::funded();
    let client = fixture.client();
    for (voter, weight) in fixture.voters.iter().zip(WEIGHTS) {
        assert_eq!(client.voting_power(voter), weight);
    }
    assert_eq!(client.total_supply(), SUPPLY);
    client.mint(&fixture.voters[2], &50);
    assert_eq!(client.voting_power(&fixture.voters[2]), WEIGHTS[2] + 50);
    assert_eq!(client.total_supply(), SUPPLY + 50);
}

#[test]
fn minting_rejects_a_non_positive_amount() {
    let fixture = Fixture::initialized();
    let result = fixture.client().try_mint(&fixture.voters[0], &0);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::InvalidAmount);
}

#[test]
fn proposing_requires_voting_power() {
    let fixture = Fixture::funded();
    let result = fixture.client().try_propose(
        &fixture.outsider,
        &fixture.title("grab"),
        &Action::SetQuorum(1),
    );
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::NoVotingPower);
}

#[test]
fn proposing_rejects_an_invalid_action() {
    let fixture = Fixture::funded();
    let result = fixture.client().try_propose(
        &fixture.voters[0],
        &fixture.title("bad"),
        &Action::SetQuorum(0),
    );
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::InvalidQuorum);
}

#[test]
fn a_new_proposal_opens_a_pending_voting_window() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(10));
    let proposal = fixture.client().proposal(&id);
    assert_eq!(proposal.status, ProposalStatus::Pending);
    assert_eq!(proposal.start, fixture.now());
    assert_eq!(proposal.end, fixture.now() + VOTING_PERIOD);
    assert_eq!(proposal.eta, 0);
    assert_eq!(proposal.supply_snapshot, SUPPLY);
}

#[test]
fn proposal_ids_increment() {
    let fixture = Fixture::funded();
    assert_eq!(fixture.propose(&Action::SetTimelock(1)), 0);
    assert_eq!(fixture.propose(&Action::SetTimelock(2)), 1);
    assert_eq!(fixture.client().proposal_count(), 2);
}

#[test]
fn votes_are_weighted_by_voting_power() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    assert_eq!(fixture.vote(0, id, true), WEIGHTS[0]);
    assert_eq!(fixture.vote(2, id, false), WEIGHTS[2]);
    let proposal = fixture.client().proposal(&id);
    assert_eq!(proposal.votes_for, WEIGHTS[0]);
    assert_eq!(proposal.votes_against, WEIGHTS[2]);
    assert_eq!(
        fixture.client().receipt(&id, &fixture.voters[0]),
        Some(WEIGHTS[0])
    );
}

#[test]
fn a_member_cannot_vote_twice() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(1, id, true);
    let result = fixture.client().try_vote(&fixture.voters[1], &id, &false);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::AlreadyVoted);
}

#[test]
fn a_member_without_power_cannot_vote() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let result = fixture.client().try_vote(&fixture.outsider, &id, &true);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::NoVotingPower);
}

#[test]
fn voting_closes_when_the_period_ends() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.end_voting();
    let result = fixture.client().try_vote(&fixture.voters[0], &id, &true);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::VotingClosed);
}

#[test]
fn queueing_while_voting_is_open_is_rejected() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(0, id, true);
    fixture.advance_time(VOTING_PERIOD - 1);
    let result = fixture.client().try_queue(&id);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::VotingOpen);
}

#[test]
fn a_proposal_below_quorum_is_defeated() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    // 200 of 1 000 votes: below the 400-vote quorum even though unanimous.
    fixture.vote(2, id, true);
    fixture.end_voting();
    assert_eq!(fixture.client().queue(&id), ProposalStatus::Defeated);
}

#[test]
fn a_proposal_losing_the_vote_is_defeated() {
    let mut fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(1, id, true);
    fixture.vote(0, id, false);
    fixture.end_voting();
    assert_eq!(fixture.client().queue(&id), ProposalStatus::Defeated);
}

#[test]
fn a_tied_vote_is_defeated() {
    let mut fixture = Fixture::funded();
    fixture.client().mint(&fixture.voters[2], &250); // 450 vs 450
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.vote(0, id, true);
    fixture.vote(2, id, false);
    fixture.end_voting();
    assert_eq!(fixture.client().queue(&id), ProposalStatus::Defeated);
}

#[test]
fn a_passed_proposal_executes_after_the_timelock() {
    let mut fixture = Fixture::funded();
    let id = fixture.queued(&Action::SetQuorum(6_000));
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    assert_eq!(fixture.status(id), ProposalStatus::Executed);
    assert_eq!(fixture.client().quorum_bps(), 6_000);
}

#[test]
fn a_mint_action_grows_the_supply() {
    let mut fixture = Fixture::funded();
    let newcomer = fixture.outsider.clone();
    let id = fixture.queued(&Action::Mint(newcomer.clone(), 500));
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    assert_eq!(fixture.client().voting_power(&newcomer), 500);
    assert_eq!(fixture.client().total_supply(), SUPPLY + 500);
}

#[test]
fn the_proposer_can_cancel_while_voting_is_open() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    fixture.client().cancel(&fixture.voters[0], &id);
    assert_eq!(fixture.status(id), ProposalStatus::Cancelled);
    let result = fixture.client().try_vote(&fixture.voters[1], &id, &true);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::NotPending);
}

#[test]
fn only_the_proposer_can_cancel() {
    let fixture = Fixture::funded();
    let id = fixture.propose(&Action::SetTimelock(1));
    let result = fixture.client().try_cancel(&fixture.voters[1], &id);
    assert_eq!(result.unwrap_err().unwrap(), GovernanceError::NotProposer);
}

#[test]
fn an_unknown_proposal_is_reported() {
    let fixture = Fixture::funded();
    let result = fixture.client().try_proposal(&42);
    assert_eq!(
        result.unwrap_err().unwrap(),
        GovernanceError::ProposalNotFound
    );
}

#[test]
fn the_full_life_cycle_runs_under_the_testnet_profile() {
    let mut fixture = Fixture::with_profile(NetworkProfile::Testnet);
    fixture.initialize();
    fixture.mint_weights();
    let id = fixture.queued(&Action::SetVotingPeriod(3_600));
    fixture.advance_time(TIMELOCK);
    fixture.client().execute(&id);
    assert_eq!(fixture.client().voting_period(), 3_600);
}
