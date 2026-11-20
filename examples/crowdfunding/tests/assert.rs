mod common;
use common::Fixture;
use soroban_sdk::testutils::Ledger as _;

#[test]
fn contribution_moves_tokens_into_escrow() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    f.client().contribute(&1, &f.backer, &250);
    assert_eq!(f.escrow_balance(), 250);
}
#[test]
fn contribution_updates_backer_record() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    f.client().contribute(&1, &f.backer, &250);
    assert_eq!(
        f.client().campaign(&1).backers.get(f.backer.clone()),
        Some(250)
    );
}
#[test]
fn contributions_accumulate() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    f.client().contribute(&1, &f.backer, &100);
    f.client().contribute(&1, &f.backer, &150);
    assert_eq!(f.client().campaign(&1).raised, 250);
}
#[test]
fn goal_reaching_allows_milestone_release() {
    let f = Fixture::new(1000);
    f.open(200, 100);
    f.client().contribute(&1, &f.backer, &200);
    assert_eq!(f.client().release_milestone(&1, &f.creator), 100);
}
#[test]
fn failed_campaign_returns_backer_contribution() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    f.client().contribute(&1, &f.backer, &200);
    f.env.ledger().set_timestamp(100);
    f.client().finalize_failure(&1);
    assert_eq!(f.client().claim_refund(&1, &f.backer), 200);
}
#[test]
fn refund_is_one_time() {
    let f = Fixture::new(1000);
    f.open(500, 100);
    f.client().contribute(&1, &f.backer, &200);
    f.env.ledger().set_timestamp(100);
    f.client().finalize_failure(&1);
    f.client().claim_refund(&1, &f.backer);
    assert_eq!(
        f.client().campaign(&1).backers.get(f.backer.clone()),
        Some(0)
    );
}
