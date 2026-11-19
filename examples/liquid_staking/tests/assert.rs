mod common;
use common::Fixture;

#[test]
fn deposit_mints_one_to_one_initial_shares() {
    let f = Fixture::new(1000);
    assert_eq!(f.client().deposit(&f.user, &400), 400);
}
#[test]
fn yield_increases_exchange_rate() {
    let f = Fixture::new(1000);
    f.client().deposit(&f.user, &500);
    f.client().accrue_yield(&f.admin, &100);
    assert_eq!(f.client().exchange_rate(), 1_200_000);
}
#[test]
fn redemption_returns_proportional_assets() {
    let f = Fixture::new(1000);
    f.client().deposit(&f.user, &500);
    f.client().accrue_yield(&f.admin, &100);
    assert_eq!(f.client().redeem(&f.user, &250), 300);
}
#[test]
fn full_redemption_returns_all_assets() {
    let f = Fixture::new(1000);
    f.client().deposit(&f.user, &500);
    f.client().accrue_yield(&f.admin, &100);
    assert_eq!(f.client().redeem(&f.user, &500), 600);
}
#[test]
fn redemption_reduces_share_balance() {
    let f = Fixture::new(1000);
    f.client().deposit(&f.user, &500);
    f.client().redeem(&f.user, &100);
    assert_eq!(f.client().shares(&f.user), 400);
}
#[test]
fn yield_is_not_minted_as_shares() {
    let f = Fixture::new(1000);
    f.client().deposit(&f.user, &500);
    f.client().accrue_yield(&f.admin, &100);
    assert_eq!(f.client().total_shares(), 500);
}
