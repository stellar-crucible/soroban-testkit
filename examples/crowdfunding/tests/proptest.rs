mod common;
use common::Fixture;
use proptest::prelude::*;
use soroban_sdk::testutils::Ledger as _;
use soroban_testkit_generators::strategies::token_amount;

proptest! { #![proptest_config(ProptestConfig::with_cases(16))]
 #[test] fn creator_cannot_withdraw_after_failed_goal(contribution in 1i128..=1000, goal in 1001i128..=2000) { let f=Fixture::new(contribution); f.open(goal,100); f.client().contribute(&1,&f.backer,&contribution); f.env.ledger().set_timestamp(100); f.client().finalize_failure(&1); let before=soroban_sdk::token::Client::new(&f.env,&f.token).balance(&f.creator); let result=f.client().try_release_milestone(&1,&f.creator); prop_assert!(result.is_err()); prop_assert_eq!(soroban_sdk::token::Client::new(&f.env,&f.token).balance(&f.creator),before); }
 #[test] fn raised_amount_never_exceeds_actual_contributions(amount in token_amount()) { prop_assume!(amount>0); let f=Fixture::new(amount); f.open(amount+1,100); f.client().contribute(&1,&f.backer,&amount); prop_assert_eq!(f.client().campaign(&1).raised,amount); }
}
