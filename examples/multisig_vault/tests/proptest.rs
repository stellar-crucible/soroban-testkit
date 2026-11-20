mod common;

use common::{Fixture, INITIAL_BALANCE, TIMELOCK};
use multisig_vault::VaultError;
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

fn withdrawal_amount() -> impl Strategy<Value = i128> {
    1i128..=INITIAL_BALANCE
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn funds_never_leave_without_threshold(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let proposal_id = fixture.propose(amount);
        fixture.advance_time(TIMELOCK);

        let result = fixture.client().try_execute(&fixture.signers[0], &proposal_id);
        prop_assert_eq!(result.unwrap_err().unwrap(), VaultError::ThresholdNotMet);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
    }

    #[test]
    fn funds_never_leave_before_timelock(
        amount in withdrawal_amount(),
        elapsed in 0u64..TIMELOCK,
    ) {
        let fixture = Fixture::funded();
        let proposal_id = fixture.ready_proposal(amount);
        fixture.advance_time(elapsed);

        let result = fixture.client().try_execute(&fixture.signers[0], &proposal_id);
        prop_assert_eq!(result.unwrap_err().unwrap(), VaultError::TimelockNotElapsed);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
    }

    #[test]
    fn valid_withdrawal_reduces_balance_exactly(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let proposal_id = fixture.ready_proposal(amount);
        fixture.advance_time(TIMELOCK);
        fixture.client().execute(&fixture.signers[0], &proposal_id);

        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE - amount);
    }

    #[test]
    fn cancelled_proposals_never_execute(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let proposal_id = fixture.ready_proposal(amount);
        fixture.client().cancel(&fixture.signers[2], &proposal_id);
        fixture.advance_time(TIMELOCK);

        let result = fixture.client().try_execute(&fixture.signers[0], &proposal_id);
        prop_assert_eq!(result.unwrap_err().unwrap(), VaultError::AlreadyCancelled);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
    }

    #[test]
    fn duplicate_approvals_never_count_twice(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let proposal_id = fixture.propose(amount);
        let client = fixture.client();
        client.approve(&fixture.signers[0], &proposal_id);

        let result = client.try_approve(&fixture.signers[0], &proposal_id);
        prop_assert_eq!(result.unwrap_err().unwrap(), VaultError::AlreadyApproved);
        prop_assert_eq!(client.proposal(&proposal_id).unwrap().approvals.len(), 1);
    }

    #[test]
    fn ready_at_is_approval_time_plus_timelock(
        amount in withdrawal_amount(),
        start in 0u64..1_000_000,
    ) {
        let fixture = Fixture::funded();
        fixture.set_time(start);
        let proposal_id = fixture.propose(amount);
        fixture.approve_threshold(proposal_id);

        let proposal = fixture.client().proposal(&proposal_id).unwrap();
        prop_assert_eq!(proposal.ready_at, start + TIMELOCK);
    }

    #[test]
    fn proposal_ids_are_monotonic(
        first in strategies::token_amount().prop_map(|a| 1 + (a % (INITIAL_BALANCE / 2))),
        second in strategies::token_amount().prop_map(|a| 1 + (a % (INITIAL_BALANCE / 2))),
    ) {
        let fixture = Fixture::funded();
        let first_id = fixture.propose(first);
        let second_id = fixture.propose(second);

        prop_assert_eq!(first_id, 0);
        prop_assert_eq!(second_id, 1);
        prop_assert_eq!(fixture.client().next_proposal_id(), 2);
    }
}
