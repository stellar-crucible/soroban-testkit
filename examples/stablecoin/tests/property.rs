//! One crate per file: this suite drives `examples/stablecoin` through
//! `soroban-testkit-generators` and `proptest`, verifying protocol-wide
//! solvency invariants — including that total stablecoin supply is strictly
//! backed by collateral above MCR across multi-asset vaults, repays,
//! redemptions, and liquidations.

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    Address, Env, Symbol,
};
use soroban_testkit_example_stablecoin::{
    PriceOracleContract, PriceOracleContractClient, StablecoinContract, StablecoinContractClient,
    Vault, BASE_REDEMPTION_FEE_BPS, BPS_SCALE, DEFAULT_MCR_BPS, FEE_DECAY_WINDOW_SECS,
    MAX_REDEMPTION_FEE_BPS, PRICE_SCALE,
};
use soroban_testkit_generators::strategies::{
    account_address, address, contract_address, ledger_sequence, timestamp, token_amount,
};

struct PropProtocol<'a> {
    oracle: PriceOracleContractClient<'a>,
    stable: StablecoinContractClient<'a>,
}

fn deploy(env: &Env, xlm_price: i128, wbtc_price: i128) -> PropProtocol<'_> {
    env.mock_all_auths();
    let admin = Address::generate(env);

    let oracle_id = env.register(PriceOracleContract, ());
    let oracle = PriceOracleContractClient::new(env, &oracle_id);
    oracle.initialize(&admin);

    let stable_id = env.register(StablecoinContract, ());
    let stable = StablecoinContractClient::new(env, &stable_id);
    stable.initialize(&admin, &oracle_id, &DEFAULT_MCR_BPS);

    let xlm = Symbol::new(env, "XLM");
    let wbtc = Symbol::new(env, "WBTC");
    oracle.set_price(&xlm, &xlm_price);
    oracle.set_price(&wbtc, &wbtc_price);
    stable.register_collateral(&xlm, &15_000);
    stable.register_collateral(&wbtc, &16_500);

    PropProtocol { oracle, stable }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn stablecoin_supply_is_strictly_backed_by_collateral_above_mcr(
        xlm_col in 10_000i128..=1_000_000_000i128,
        wbtc_col in 10_000i128..=1_000_000_000i128,
        xlm_price in 1_000_000i128..=50_000_000i128,
        wbtc_price in 100_000_000i128..=5_000_000_000i128,
        xlm_util in 1i128..=100i128,
        wbtc_util in 1i128..=100i128,
        repay_pct in 0i128..=50i128,
        borrower_a in account_address(),
        borrower_b in contract_address(),
    ) {
        let env = Env::default();
        let proto = deploy(&env, xlm_price, wbtc_price);
        let xlm = Symbol::new(&env, "XLM");
        let wbtc = Symbol::new(&env, "WBTC");
        let addr_a = borrower_a.to_address(&env);
        let addr_b = borrower_b.to_address(&env);

        let xlm_val = xlm_col * xlm_price / PRICE_SCALE;
        let max_xlm_mint = xlm_val * BPS_SCALE / 15_000;
        let xlm_mint = (max_xlm_mint * xlm_util / 100).max(1);

        let wbtc_val = wbtc_col * wbtc_price / PRICE_SCALE;
        let max_wbtc_mint = wbtc_val * BPS_SCALE / 16_500;
        let wbtc_mint = (max_wbtc_mint * wbtc_util / 100).max(1);

        proto.stable.deposit_collateral(&addr_a, &xlm, &xlm_col);
        proto.stable.mint(&addr_a, &xlm, &xlm_mint);

        proto.stable.deposit_collateral(&addr_b, &wbtc, &wbtc_col);
        proto.stable.mint(&addr_b, &wbtc, &wbtc_mint);

        let repay_amt = xlm_mint * repay_pct / 100;
        if repay_amt > 0 {
            proto.stable.repay(&addr_a, &xlm, &repay_amt);
        }

        let vault_a = proto.stable.vault(&addr_a, &xlm);
        let vault_b = proto.stable.vault(&addr_b, &wbtc);
        let total_supply = proto.stable.total_supply();
        let backing_a = vault_a.collateral * xlm_price / PRICE_SCALE;
        let backing_b = vault_b.collateral * wbtc_price / PRICE_SCALE;
        let total_backing = backing_a + backing_b;

        // Global solvency: every circulating stablecoin is strictly backed above MCR.
        prop_assert_eq!(total_supply, vault_a.debt + vault_b.debt);
        prop_assert!(total_backing * BPS_SCALE >= total_supply * DEFAULT_MCR_BPS);

        if vault_a.debt > 0 {
            prop_assert!(proto.stable.collateral_ratio_bps(&addr_a, &xlm) >= 15_000);
        }
        if vault_b.debt > 0 {
            prop_assert!(proto.stable.collateral_ratio_bps(&addr_b, &wbtc) >= 16_500);
        }
    }

    #[test]
    fn mint_and_repay_preserve_supply_and_debt_conservation(
        raw_col in token_amount(),
        mint_pct in 1i128..=100i128,
        repay_pct in 0i128..=100i128,
        owner in account_address(),
    ) {
        prop_assume!(raw_col >= 1_000);

        let env = Env::default();
        let proto = deploy(&env, PRICE_SCALE, 50_000 * PRICE_SCALE);
        let xlm = Symbol::new(&env, "XLM");
        let owner = owner.to_address(&env);

        let max_mint = raw_col * BPS_SCALE / 15_000;
        let minted = (max_mint * mint_pct / 100).max(1);
        let repaid = minted * repay_pct / 100;

        proto.stable.deposit_collateral(&owner, &xlm, &raw_col);
        proto.stable.mint(&owner, &xlm, &minted);
        if repaid > 0 {
            proto.stable.repay(&owner, &xlm, &repaid);
        }

        let expected_debt = minted - repaid;
        prop_assert_eq!(proto.stable.total_supply(), expected_debt);
        prop_assert_eq!(proto.stable.balance_of(&owner), expected_debt);
        prop_assert_eq!(
            proto.stable.vault(&owner, &xlm),
            Vault {
                collateral: raw_col,
                debt: expected_debt,
            }
        );
    }

    #[test]
    fn multi_asset_collateral_deposits_and_safe_withdrawals_preserve_mcr(
        raw_col in token_amount(),
        withdraw_pct in 1i128..=99i128,
        owner in contract_address(),
    ) {
        prop_assume!(raw_col >= 10_000);

        let env = Env::default();
        let proto = deploy(&env, PRICE_SCALE, 50_000 * PRICE_SCALE);
        let xlm = Symbol::new(&env, "XLM");
        let owner = owner.to_address(&env);

        let debt = raw_col / 3;
        let min_col = debt * 15_000 / BPS_SCALE + 1;
        let excess = raw_col - min_col;
        let withdraw = (excess * withdraw_pct / 100).max(1);

        proto.stable.deposit_collateral(&owner, &xlm, &raw_col);
        proto.stable.mint(&owner, &xlm, &debt);
        proto.stable.withdraw_collateral(&owner, &xlm, &withdraw);

        let vault = proto.stable.vault(&owner, &xlm);
        prop_assert_eq!(vault.collateral, raw_col - withdraw);
        prop_assert!(proto.stable.collateral_ratio_bps(&owner, &xlm) >= 15_000);
    }

    #[test]
    fn stability_pool_staking_and_unstaking_conserve_stablecoin_balances(
        raw_col in token_amount(),
        stake_pct in 1i128..=100i128,
        unstake_pct in 1i128..=100i128,
        staker in address(),
    ) {
        prop_assume!(raw_col >= 10_000);

        let env = Env::default();
        let proto = deploy(&env, PRICE_SCALE, 50_000 * PRICE_SCALE);
        let xlm = Symbol::new(&env, "XLM");
        let staker = staker.to_address(&env);

        let minted = raw_col / 2;
        let staked = (minted * stake_pct / 100).max(1);
        let unstaked = (staked * unstake_pct / 100).max(1);

        proto.stable.deposit_collateral(&staker, &xlm, &raw_col);
        proto.stable.mint(&staker, &xlm, &minted);
        proto.stable.stake_stability_pool(&staker, &staked);
        proto.stable.unstake_stability_pool(&staker, &unstaked);

        let remaining_stake = staked - unstaked;
        prop_assert_eq!(proto.stable.stability_stake_of(&staker), remaining_stake);
        prop_assert_eq!(proto.stable.stability_pool_total(), remaining_stake);
        prop_assert_eq!(
            proto.stable.balance_of(&staker) + proto.stable.stability_stake_of(&staker),
            minted
        );
    }

    #[test]
    fn algorithmic_redemption_fee_stays_within_floor_and_cap_across_timestamps(
        raw_col in token_amount(),
        redeem_pct in 1i128..=50i128,
        sequence in ledger_sequence(),
        when in timestamp(),
        wait_hours in 0u64..=1_000u64,
    ) {
        prop_assume!(raw_col >= 10_000);

        let env = Env::default();
        env.ledger().set_sequence_number(sequence);
        env.ledger().set_timestamp(when);
        let proto = deploy(&env, PRICE_SCALE, 50_000 * PRICE_SCALE);
        let xlm = Symbol::new(&env, "XLM");
        let holder = Address::generate(&env);

        let debt = raw_col / 2;
        let redeem_amt = (debt * redeem_pct / 100).max(1);

        proto.stable.deposit_collateral(&holder, &xlm, &raw_col);
        proto.stable.mint(&holder, &xlm, &debt);

        let quoted = proto.stable.quote_redemption_fee_bps(&redeem_amt);
        prop_assert!(quoted >= BASE_REDEMPTION_FEE_BPS);
        prop_assert!(quoted <= MAX_REDEMPTION_FEE_BPS);

        proto.stable.redeem(&holder, &holder, &xlm, &redeem_amt);
        let after_redeem_base = proto.stable.base_rate_bps();
        prop_assert!(after_redeem_base >= 0);
        prop_assert!(after_redeem_base <= MAX_REDEMPTION_FEE_BPS - BASE_REDEMPTION_FEE_BPS);

        env.ledger()
            .set_timestamp(when.saturating_add(wait_hours * FEE_DECAY_WINDOW_SECS));
        let decayed = proto.stable.base_rate_bps();
        prop_assert!(decayed >= 0);
        prop_assert!(decayed <= after_redeem_base);
    }

    #[test]
    fn redemption_never_degrades_vault_collateralization_ratio(
        collateral in 10_000i128..=100_000_000i128,
        mint_pct in 10i128..=100i128,
        redeem_pct in 1i128..=90i128,
        borrower in account_address(),
        redeemer in contract_address(),
    ) {
        let env = Env::default();
        let xlm_price = 2 * PRICE_SCALE;
        let proto = deploy(&env, xlm_price, 50_000 * PRICE_SCALE);
        let xlm = Symbol::new(&env, "XLM");
        let borrower = borrower.to_address(&env);
        let redeemer = redeemer.to_address(&env);

        let max_mint = collateral * 2 * BPS_SCALE / 15_000;
        let debt = (max_mint * mint_pct / 100).max(100);
        let redeem_amt = (debt * redeem_pct / 100).max(1);

        proto.stable.deposit_collateral(&borrower, &xlm, &collateral);
        proto.stable.mint(&borrower, &xlm, &debt);
        proto.stable.deposit_collateral(&redeemer, &xlm, &collateral);
        proto.stable.mint(&redeemer, &xlm, &debt);

        let cr_before = proto.stable.collateral_ratio_bps(&borrower, &xlm);
        proto.stable.redeem(&redeemer, &borrower, &xlm, &redeem_amt);
        let cr_after = proto.stable.collateral_ratio_bps(&borrower, &xlm);

        prop_assert!(cr_after >= cr_before);
        prop_assert!(cr_after >= DEFAULT_MCR_BPS);
    }

    #[test]
    fn liquidation_absorbs_full_underwater_debt_and_seizes_collateral_into_pool(
        collateral in 15_000i128..=10_000_000i128,
        borrower in account_address(),
        staker in contract_address(),
    ) {
        let env = Env::default();
        let proto = deploy(&env, PRICE_SCALE, 50_000 * PRICE_SCALE);
        let xlm = Symbol::new(&env, "XLM");
        let wbtc = Symbol::new(&env, "WBTC");
        let borrower = borrower.to_address(&env);
        let staker = staker.to_address(&env);
        let liquidator = Address::generate(&env);

        let debt = collateral * BPS_SCALE / 15_000;
        proto.stable.deposit_collateral(&borrower, &xlm, &collateral);
        proto.stable.mint(&borrower, &xlm, &debt);

        // Staker collateralizes with WBTC and funds the Stability Pool with `debt`.
        proto.stable.deposit_collateral(&staker, &wbtc, &collateral);
        proto.stable.mint(&staker, &wbtc, &debt);
        proto.stable.stake_stability_pool(&staker, &debt);

        // Drop XLM price to $0.75 so borrower falls below 150% MCR.
        proto.oracle.set_price(&xlm, &(75 * PRICE_SCALE / 100));

        let seized = proto.stable.liquidate(&liquidator, &borrower, &xlm);
        prop_assert_eq!(seized, collateral);
        prop_assert_eq!(
            proto.stable.vault(&borrower, &xlm),
            Vault {
                collateral: 0,
                debt: 0,
            }
        );
        prop_assert_eq!(proto.stable.pool_collateral_of(&xlm), collateral);
        prop_assert_eq!(proto.stable.stability_pool_total(), 0);
        prop_assert_eq!(proto.stable.total_supply(), debt);
    }
}
