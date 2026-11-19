mod common;
use common::Fixture;

#[test] fn fixture_initializes_pool(){let f=Fixture::new(1000);assert_eq!(f.client().total_assets(),0);}
#[test] fn fixture_records_initial_exchange_rate(){let f=Fixture::new(1000);assert_eq!(f.client().exchange_rate(),1_000_000);}
#[test] fn fixture_asset_is_funded(){let f=Fixture::new(1000);assert_eq!(f.underlying(&f.user),1000);}
#[test] fn initial_supply_is_zero(){let f=Fixture::new(1000);assert_eq!(f.client().total_shares(),0);}
#[test] fn deposit_transfers_underlying(){let f=Fixture::new(1000);f.client().deposit(&f.user,&500);assert_eq!(f.underlying(&f.user),500);}
