mod common;
use common::Fixture;

#[test]
fn fixtures_deploy_and_fund_roles() {
    let f = Fixture::new(500);
    assert_eq!(f.backer_balance(), 500);
}
#[test]
fn campaign_starts_empty() {
    let f = Fixture::new(500);
    f.open(300, 100);
    let c = f.client().campaign(&1);
    assert_eq!(c.raised, 0);
}
#[test]
fn campaign_records_goal() {
    let f = Fixture::new(500);
    f.open(300, 100);
    assert_eq!(f.client().campaign(&1).goal, 300);
}
#[test]
fn campaign_records_deadline() {
    let f = Fixture::new(500);
    f.open(300, 100);
    assert_eq!(f.client().campaign(&1).deadline, 100);
}
#[test]
fn distinct_campaign_ids_are_supported() {
    let f = Fixture::new(500);
    f.open(300, 100);
    f.client().create(&2, &f.creator, &f.token, &200, &100, &3);
    assert_eq!(f.client().campaign(&2).goal, 200);
}
