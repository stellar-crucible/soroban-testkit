//! Property tests driven by `soroban-testkit-generators`: generated voter
//! sets for quorum maths, weight conservation and timelock enforcement.

mod common;

use common::{Fixture, QUORUM_BPS, TIMELOCK, VOTING_PERIOD};
use governance::{Action, GovernanceError, ProposalStatus, MAX_BPS};
use proptest::prelude::*;
use soroban_sdk::Address;
use soroban_testkit_generators::strategies::{account_address, AddressBytes};

/// One generated member: an account address, a weight and how they vote
/// (`None` abstains).
fn member() -> impl Strategy<Value = (AddressBytes, i128, Option<bool>)> {
    (
        account_address(),
        1i128..=1_000_000,
        prop_oneof![Just(None), Just(Some(true)), Just(Some(false))],
    )
}

fn members() -> impl Strategy<Value = Vec<(AddressBytes, i128, Option<bool>)>> {
    prop::collection::vec(member(), 1..=8)
}

/// Mint every member's weight and open a proposal from the heaviest one.
fn open(fixture: &Fixture, members: &[(AddressBytes, i128, Option<bool>)]) -> (Vec<Address>, u64) {
    let env = &fixture.ctx.env;
    let addresses: Vec<Address> = members
        .iter()
        .map(|(address, _, _)| address.to_address(env))
        .collect();
    for (address, (_, weight, _)) in addresses.iter().zip(members) {
        fixture.client().mint(address, weight);
    }
    let id = fixture.client().propose(
        &addresses[0],
        &fixture.title("generated"),
        &Action::SetTimelock(7),
    );
    (addresses, id)
}

fn cast(
    fixture: &Fixture,
    id: u64,
    addresses: &[Address],
    members: &[(AddressBytes, i128, Option<bool>)],
) {
    for (address, (_, _, choice)) in addresses.iter().zip(members) {
        if let Some(support) = choice {
            fixture.client().vote(address, &id, support);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn quorum_outcome_matches_the_reference_model(members in members()) {
        let mut fixture = Fixture::initialized();
        let (addresses, id) = open(&fixture, &members);
        cast(&fixture, id, &addresses, &members);
        fixture.end_voting();

        let supply: i128 = members.iter().map(|(_, weight, _)| weight).sum();
        let votes_for: i128 = members.iter().filter(|m| m.2 == Some(true)).map(|m| m.1).sum();
        let votes_against: i128 = members.iter().filter(|m| m.2 == Some(false)).map(|m| m.1).sum();
        let quorum = (supply * QUORUM_BPS as i128 + (MAX_BPS as i128 - 1)) / MAX_BPS as i128;
        let expected = if votes_for + votes_against >= quorum && votes_for > votes_against {
            ProposalStatus::Queued
        } else {
            ProposalStatus::Defeated
        };

        prop_assert_eq!(fixture.client().quorum_votes(&id), quorum);
        prop_assert_eq!(fixture.client().queue(&id), expected);
    }

    #[test]
    fn vote_weight_is_conserved(members in members()) {
        let fixture = Fixture::initialized();
        let (addresses, id) = open(&fixture, &members);
        cast(&fixture, id, &addresses, &members);

        let proposal = fixture.client().proposal(&id);
        let cast_weight: i128 = members.iter().filter(|m| m.2.is_some()).map(|m| m.1).sum();
        prop_assert_eq!(proposal.votes_for + proposal.votes_against, cast_weight);
        prop_assert!(cast_weight <= proposal.supply_snapshot);
        prop_assert_eq!(proposal.supply_snapshot, fixture.client().total_supply());

        // Receipts sum to the same total.
        let receipts: i128 = addresses
            .iter()
            .filter_map(|address| fixture.client().receipt(&id, address))
            .sum();
        prop_assert_eq!(receipts, cast_weight);
    }

    #[test]
    fn a_repeated_vote_never_changes_the_tally(members in members(), support in any::<bool>()) {
        let fixture = Fixture::initialized();
        let (addresses, id) = open(&fixture, &members);
        cast(&fixture, id, &addresses, &members);
        let before = fixture.client().proposal(&id);
        for (address, (_, _, choice)) in addresses.iter().zip(&members) {
            if choice.is_some() {
                let result = fixture.client().try_vote(address, &id, &support);
                prop_assert_eq!(result.unwrap_err().unwrap(), GovernanceError::AlreadyVoted);
            }
        }
        prop_assert_eq!(fixture.client().proposal(&id), before);
    }

    #[test]
    fn execution_waits_for_the_whole_timelock(elapsed in 0u64..TIMELOCK) {
        let mut fixture = Fixture::funded();
        let id = fixture.queued(&Action::SetQuorum(7_000));
        fixture.advance_time(elapsed);
        let result = fixture.client().try_execute(&id);
        prop_assert_eq!(result.unwrap_err().unwrap(), GovernanceError::TimelockNotElapsed);
        prop_assert_eq!(fixture.client().quorum_bps(), QUORUM_BPS);
    }

    #[test]
    fn execution_succeeds_any_time_after_the_timelock(extra in 0u64..=10 * VOTING_PERIOD) {
        let mut fixture = Fixture::funded();
        let id = fixture.queued(&Action::SetQuorum(7_000));
        fixture.advance_time(TIMELOCK + extra);
        fixture.client().execute(&id);
        prop_assert_eq!(fixture.status(id), ProposalStatus::Executed);
        prop_assert_eq!(fixture.client().quorum_bps(), 7_000);
    }

    #[test]
    fn votes_are_refused_at_or_after_the_deadline(late in 0u64..=VOTING_PERIOD) {
        let mut fixture = Fixture::funded();
        let id = fixture.propose(&Action::SetTimelock(1));
        fixture.advance_time(VOTING_PERIOD + late);
        let result = fixture.client().try_vote(&fixture.voters[0], &id, &true);
        prop_assert_eq!(result.unwrap_err().unwrap(), GovernanceError::VotingClosed);
    }
}
