extern crate std;

// This file exercises `soroban-testkit-fixtures` on its own: how
// `TestContextBuilder` provisions borrowers and liquidators, how the ledger
// clock drives algorithmic redemption fee decay, and how `reset` clears
// protocol state while keeping every actor address intact.
use soroban_sdk::{Address, Symbol};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

use crate::{
    PriceOracleContract, PriceOracleContractClient, StablecoinContract, StablecoinContractClient,
    Vault, BASE_REDEMPTION_FEE_BPS, DEFAULT_MCR_BPS, FEE_DECAY_WINDOW_SECS, PRICE_SCALE,
};

fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

struct Protocol<'a> {
    oracle_id: Address,
    oracle: PriceOracleContractClient<'a>,
    stablecoin: StablecoinContractClient<'a>,
}

fn deploy(ctx: &TestContext) -> Protocol<'_> {
    let oracle_id = ctx.env.register(PriceOracleContract, ());
    let oracle = PriceOracleContractClient::new(&ctx.env, &oracle_id);
    oracle.initialize(&ctx.admin);

    let stable_id = ctx.env.register(StablecoinContract, ());
    let stablecoin = StablecoinContractClient::new(&ctx.env, &stable_id);
    stablecoin.initialize(&ctx.admin, &oracle_id, &DEFAULT_MCR_BPS);

    let xlm = Symbol::new(&ctx.env, "XLM");
    let wbtc = Symbol::new(&ctx.env, "WBTC");
    oracle.set_price(&xlm, &PRICE_SCALE);
    oracle.set_price(&wbtc, &(50_000 * PRICE_SCALE));
    stablecoin.register_collateral(&xlm, &15_000);
    stablecoin.register_collateral(&wbtc, &16_000);

    Protocol {
        oracle_id,
        oracle,
        stablecoin,
    }
}

#[test]
fn initialize_records_admin_oracle_and_mcr_at_zero_supply() {
    let ctx = context(1);
    let proto = deploy(&ctx);

    assert_eq!(proto.stablecoin.admin(), ctx.admin);
    assert_eq!(proto.stablecoin.oracle(), proto.oracle_id);
    assert_eq!(proto.stablecoin.mcr_bps(), DEFAULT_MCR_BPS);
    assert_eq!(proto.stablecoin.total_supply(), 0);
    assert_eq!(proto.stablecoin.balance_of(ctx.user(0)), 0);
    assert_eq!(proto.stablecoin.stability_pool_total(), 0);
}

#[test]
fn multi_asset_collateral_deposits_and_withdrawals_track_each_asset_independently() {
    let ctx = context(1);
    let proto = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");
    let wbtc = Symbol::new(&ctx.env, "WBTC");

    proto.stablecoin.deposit_collateral(&borrower, &xlm, &3_000);
    proto.stablecoin.deposit_collateral(&borrower, &wbtc, &2);
    proto.stablecoin.withdraw_collateral(&borrower, &xlm, &500);

    assert_eq!(
        proto.stablecoin.vault(&borrower, &xlm),
        Vault {
            collateral: 2_500,
            debt: 0,
        }
    );
    assert_eq!(
        proto.stablecoin.vault(&borrower, &wbtc),
        Vault {
            collateral: 2,
            debt: 0,
        }
    );
}

#[test]
fn minting_and_repaying_stablecoin_moves_supply_and_vault_debt_together() {
    let ctx = context(1);
    let proto = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    proto.stablecoin.deposit_collateral(&borrower, &xlm, &1_500);
    let supply = proto.stablecoin.mint(&borrower, &xlm, &1_000);

    assert_eq!(supply, 1_000);
    assert_eq!(proto.stablecoin.balance_of(&borrower), 1_000);
    assert_eq!(
        proto.stablecoin.collateral_ratio_bps(&borrower, &xlm),
        15_000
    );

    let remaining = proto.stablecoin.repay(&borrower, &xlm, &400);
    assert_eq!(remaining, 600);
    assert_eq!(proto.stablecoin.total_supply(), 600);
    assert_eq!(proto.stablecoin.balance_of(&borrower), 600);
    assert_eq!(
        proto.stablecoin.collateral_ratio_bps(&borrower, &xlm),
        25_000
    );
}

#[test]
#[should_panic(expected = "mint breaches MCR")]
fn minting_above_mcr_limit_is_rejected() {
    let ctx = context(1);
    let proto = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    proto.stablecoin.deposit_collateral(&borrower, &xlm, &1_500);
    proto.stablecoin.mint(&borrower, &xlm, &1_001);
}

#[test]
fn stability_pool_staking_and_unstaking_adjusts_pool_depth() {
    let ctx = context(1);
    let proto = deploy(&ctx);
    let staker = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    proto.stablecoin.deposit_collateral(&staker, &xlm, &3_000);
    proto.stablecoin.mint(&staker, &xlm, &2_000);

    let after_stake = proto.stablecoin.stake_stability_pool(&staker, &1_500);
    assert_eq!(after_stake, 1_500);
    assert_eq!(proto.stablecoin.stability_stake_of(&staker), 1_500);
    assert_eq!(proto.stablecoin.balance_of(&staker), 500);

    let after_unstake = proto.stablecoin.unstake_stability_pool(&staker, &500);
    assert_eq!(after_unstake, 1_000);
    assert_eq!(proto.stablecoin.stability_stake_of(&staker), 1_000);
    assert_eq!(proto.stablecoin.balance_of(&staker), 1_000);
}

#[test]
fn oracle_price_drop_enables_stability_pool_liquidation() {
    let ctx = context(3);
    let proto = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let staker = ctx.user(1).clone();
    let liquidator = ctx.user(2).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");
    let wbtc = Symbol::new(&ctx.env, "WBTC");

    // Borrower opens an XLM vault right at 150% MCR.
    proto.stablecoin.deposit_collateral(&borrower, &xlm, &1_500);
    proto.stablecoin.mint(&borrower, &xlm, &1_000);

    // Staker mints against WBTC and funds the Stability Pool.
    proto.stablecoin.deposit_collateral(&staker, &wbtc, &1);
    proto.stablecoin.mint(&staker, &wbtc, &1_200);
    proto.stablecoin.stake_stability_pool(&staker, &1_200);

    // XLM drops from $1.00 to $0.80, taking the borrower's CR to 120%.
    proto.oracle.set_price(&xlm, &(8 * PRICE_SCALE / 10));
    assert_eq!(
        proto.stablecoin.collateral_ratio_bps(&borrower, &xlm),
        12_000
    );

    let seized = proto.stablecoin.liquidate(&liquidator, &borrower, &xlm);
    assert_eq!(seized, 1_500);
    assert_eq!(proto.stablecoin.pool_collateral_of(&xlm), 1_500);
    assert_eq!(proto.stablecoin.stability_pool_total(), 200);
    assert_eq!(proto.stablecoin.total_supply(), 1_200);
    assert_eq!(
        proto.stablecoin.vault(&borrower, &xlm),
        Vault {
            collateral: 0,
            debt: 0,
        }
    );
}

#[test]
#[should_panic(expected = "vault is healthy")]
fn liquidating_a_healthy_vault_is_rejected() {
    let ctx = context(2);
    let proto = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let liquidator = ctx.user(1).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    proto.stablecoin.deposit_collateral(&borrower, &xlm, &2_000);
    proto.stablecoin.mint(&borrower, &xlm, &1_000);
    proto.stablecoin.stake_stability_pool(&borrower, &1_000);

    proto.stablecoin.liquidate(&liquidator, &borrower, &xlm);
}

#[test]
fn algorithmic_redemption_fee_increases_on_redemption_and_decays_with_ledger_clock() {
    let mut ctx = context(2);
    ctx.set_timestamp(1_700_000_000);
    let proto = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let redeemer = ctx.user(1).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    // Mint 2_000 total supply (1_000 each) so redeeming 100 is 5% of supply -> +250 bps base rate.
    proto.stablecoin.deposit_collateral(&borrower, &xlm, &2_000);
    proto.stablecoin.mint(&borrower, &xlm, &1_000);
    proto.stablecoin.deposit_collateral(&redeemer, &xlm, &2_000);
    proto.stablecoin.mint(&redeemer, &xlm, &1_000);

    assert_eq!(proto.stablecoin.base_rate_bps(), 0);
    assert_eq!(proto.stablecoin.quote_redemption_fee_bps(&100), 300);

    let collateral_out = proto.stablecoin.redeem(&redeemer, &borrower, &xlm, &100);
    assert_eq!(collateral_out, 97);
    assert_eq!(proto.stablecoin.base_rate_bps(), 250);

    // Advance ledger time by 100 hours: base rate decays by 100 bps.
    ctx.advance_time(FEE_DECAY_WINDOW_SECS * 100);
    assert_eq!(proto.stablecoin.base_rate_bps(), 150);

    // Advance another 200 hours: base rate decays all the way back to zero (floor fee only).
    ctx.advance_time(FEE_DECAY_WINDOW_SECS * 200);
    assert_eq!(proto.stablecoin.base_rate_bps(), 0);
    let floor_fee = proto.stablecoin.quote_redemption_fee_bps(&0);
    assert_eq!(floor_fee, BASE_REDEMPTION_FEE_BPS);
}

#[test]
fn reset_keeps_borrowers_and_clears_protocol_state() {
    let mut ctx = context(1);
    let late = ctx.add_user().clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    {
        let proto = deploy(&ctx);
        proto.stablecoin.deposit_collateral(&late, &xlm, &1_500);
        proto.stablecoin.mint(&late, &xlm, &1_000);
        assert_eq!(proto.stablecoin.total_supply(), 1_000);
    }

    ctx.reset();
    let proto = deploy(&ctx);
    let xlm = Symbol::new(&ctx.env, "XLM");

    assert_eq!(ctx.users.len(), 2);
    assert_eq!(proto.stablecoin.total_supply(), 0);
    assert_eq!(proto.stablecoin.balance_of(ctx.user(1)), 0);
    assert_eq!(
        proto.stablecoin.vault(ctx.user(1), &xlm),
        Vault {
            collateral: 0,
            debt: 0,
        }
    );
}
