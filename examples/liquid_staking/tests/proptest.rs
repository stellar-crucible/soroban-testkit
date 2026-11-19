mod common;
use common::Fixture;
use proptest::prelude::*;
use soroban_testkit_generators::strategies::token_amount;

proptest! { #![proptest_config(ProptestConfig::with_cases(32))]
 #[test] fn redemption_value_monotonically_increases_with_yield(deposit in 2i128..=1_000_000, first in 0i128..=100_000, second in 1i128..=100_000) { let before=Fixture::new(deposit); before.client().deposit(&before.user,&deposit); before.client().accrue_yield(&before.admin,&first); let before_value=before.client().redeem(&before.user,&deposit); let after=Fixture::new(deposit); after.client().deposit(&after.user,&deposit); after.client().accrue_yield(&after.admin,&(first+second)); let after_value=after.client().redeem(&after.user,&deposit); prop_assert!(after_value>=before_value); }
 #[test] fn total_underlying_covers_depositor_claims(deposit in token_amount(), reward in 0i128..=1_000_000) { prop_assume!(deposit>0); let f=Fixture::new(deposit); f.client().deposit(&f.user,&deposit); f.client().accrue_yield(&f.admin,&reward); let shares=f.client().shares(&f.user); let value=f.client().redeem(&f.user,&shares); prop_assert_eq!(value,deposit+reward); }
}
