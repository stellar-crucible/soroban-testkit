mod common;

use common::{Fixture, INITIAL_BALANCE, TIMELOCK};
use guardian_treasury::TreasuryError;
use proptest::prelude::*;
use soroban_sdk::testutils::Ledger as _;
use soroban_testkit_generators::strategies;

fn withdrawal_amount() -> impl Strategy<Value = i128> {
    1i128..=INITIAL_BALANCE
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn guardian_veto_makes_execution_impossible(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let id = fixture.ready_proposal(amount);
        fixture.client().veto(&fixture.guardian, &id);
        fixture.advance_time(TIMELOCK);
        let result = fixture.client().try_execute(&fixture.signers[0], &id);
        prop_assert_eq!(result.unwrap_err().unwrap(), TreasuryError::AlreadyVetoed);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
    }

    #[test]
    fn veto_never_debits_any_amount(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let id = fixture.ready_proposal(amount);
        fixture.client().veto(&fixture.guardian, &id);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
        prop_assert!(fixture.client().proposal(&id).unwrap().vetoed);
    }

    #[test]
    fn execution_is_blocked_until_the_exact_deadline(
        amount in withdrawal_amount(),
        elapsed in 0u64..TIMELOCK,
    ) {
        let fixture = Fixture::funded();
        let id = fixture.ready_proposal(amount);
        fixture.advance_time(elapsed);
        let result = fixture.client().try_execute(&fixture.signers[0], &id);
        prop_assert_eq!(result.unwrap_err().unwrap(), TreasuryError::TimelockNotElapsed);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
    }

    #[test]
    fn threshold_approval_starts_timelock_from_last_approval(
        amount in withdrawal_amount(),
        timestamp in 0u64..1_000_000,
    ) {
        let fixture = Fixture::funded();
        fixture.ctx.env.ledger().set_timestamp(timestamp);
        let id = fixture.propose(amount);
        fixture.approve_threshold(id);
        let proposal = fixture.client().proposal(&id).unwrap();
        prop_assert_eq!(proposal.ready_at, timestamp + TIMELOCK);
    }

    #[test]
    fn unique_approvals_are_required_for_threshold(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let id = fixture.propose(amount);
        let client = fixture.client();
        client.approve(&fixture.signers[0], &id);
        let duplicate = client.try_approve(&fixture.signers[0], &id);
        prop_assert_eq!(duplicate.unwrap_err().unwrap(), TreasuryError::AlreadyApproved);
        prop_assert_eq!(client.proposal(&id).unwrap().approvals.len(), 1);
    }

    #[test]
    fn valid_execution_debits_exactly_the_approved_amount(
        amount in strategies::token_amount().prop_map(|value| 1 + (value % INITIAL_BALANCE)),
    ) {
        let fixture = Fixture::funded();
        let id = fixture.ready_proposal(amount);
        fixture.advance_time(TIMELOCK);
        fixture.client().execute(&fixture.signers[0], &id);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE - amount);
    }
}
