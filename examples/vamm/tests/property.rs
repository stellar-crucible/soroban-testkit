extern crate std;

use proptest::prelude::*;
use soroban_sdk::{testutils::Address as _, Address, Env};
use soroban_testkit_example_vamm::{
    liquidation_required, Position, VammContract, VammContractClient,
};

fn setup() -> (Env, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    let contract = env.register(VammContract, ());
    let client = VammContractClient::new(&env, &contract);
    client.initialize(&10_000_000, &10_000_000);
    let trader = Address::generate(&env);
    (env, contract, trader)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn leveraged_open_size_is_the_notional_in_virtual_units(
        collateral in 1i128..=100_000,
        leverage in 1i32..=10,
    ) {
        let (env, contract, trader) = setup();
        let client = VammContractClient::new(&env, &contract);
        let position = client.open_position(&trader, &1, &collateral, &leverage);
        prop_assert_eq!(position.size, collateral.saturating_mul(leverage as i128));
    }

    #[test]
    fn underwater_positions_are_always_liquidatable(
        collateral in 1i128..=10_000,
        mark in 10_000_000i128..=12_000_000,
    ) {
        let position = Position {
            trader: Address::generate(&Env::default()),
            size: -20_000_000,
            collateral,
            entry_mark: 10_000_000,
            leverage: 2,
        };
        prop_assert!(liquidation_required(&position, mark));
    }

    #[test]
    fn short_and_long_have_opposite_pnl_sign(
        collateral in 1i128..=100_000,
        leverage in 1i32..=10,
    ) {
        let (env, contract, trader) = setup();
        let client = VammContractClient::new(&env, &contract);
        let long = client.open_position(&trader, &1, &collateral, &leverage);
        client.move_mark(&10_000_000, &12_000_000);
        let long_pnl = soroban_testkit_example_vamm::unrealized_pnl(&long, 12_000_000);
        prop_assert!(long_pnl > 0);

        let env2 = Env::default();
        env2.mock_all_auths();
        let contract2 = env2.register(VammContract, ());
        let client2 = VammContractClient::new(&env2, &contract2);
        client2.initialize(&10_000_000, &10_000_000);
        let trader2 = Address::generate(&env2);
        let short = client2.open_position(&trader2, &-1, &collateral, &leverage);
        let short_pnl = soroban_testkit_example_vamm::unrealized_pnl(&short, 12_000_000);
        prop_assert!(short_pnl < 0);
    }

    #[test]
    fn close_never_returns_an_explosive_settlement(
        collateral in 1i128..=100_000,
        leverage in 1i32..=10,
    ) {
        let (env, contract, trader) = setup();
        let client = VammContractClient::new(&env, &contract);
        client.open_position(&trader, &1, &collateral, &leverage);
        let settlement = client.close_position(&trader);
        prop_assert!(settlement >= collateral);
    }

    #[test]
    fn margin_increase_never_reduces_collateral(
        initial in 1i128..=10_000,
        added in 1i128..=10_000,
    ) {
        let (env, contract, trader) = setup();
        let client = VammContractClient::new(&env, &contract);
        let position = client.open_position(&trader, &1, &initial, &2);
        let updated = client.add_margin(&trader, &added);
        prop_assert_eq!(updated.collateral, position.collateral + added);
    }

    #[test]
    fn liquidation_clears_the_open_position(mark in 1i128..=50_000_000) {
        let (env, contract, trader) = setup();
        let client = VammContractClient::new(&env, &contract);
        client.open_position(&trader, &1, &100, &10);
        client.move_mark(&10_000_000, &mark);
        if client.is_liquidatable(&trader) {
            client.liquidate(&trader);
            prop_assert!(client.position(&trader).is_none());
        } else {
            prop_assert!(client.position(&trader).is_some());
        }
    }
}
