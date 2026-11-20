extern crate std;

use soroban_sdk::testutils::Ledger as _;
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::Address;
use soroban_testkit_example_dynamic_amm::{
    DynamicAmmContract, DynamicAmmContractClient, DynamicAmmError, DEFAULT_BASE_FEE_BPS,
    DEFAULT_MAX_FEE_BPS, DEFAULT_MIN_FEE_BPS, DEFAULT_TWAP_WINDOW, DEFAULT_VOLATILITY_FACTOR,
    PRICE_PRECISION,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const ONE: i128 = 10_000_000;

struct AmmFixture {
    ctx: TestContext,
    pool: Address,
    token_a: Address,
    token_b: Address,
    lp_1: Address,
    lp_2: Address,
    trader: Address,
}

fn setup() -> AmmFixture {
    let ctx = TestContextBuilder::new().with_users(3).build();

    let pool = ctx.env.register(DynamicAmmContract, ());
    let token_a = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    let token_b = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let lp_1 = ctx.user(0).clone();
    let lp_2 = ctx.user(1).clone();
    let trader = ctx.user(2).clone();

    for user in [&lp_1, &lp_2, &trader] {
        StellarAssetClient::new(&ctx.env, &token_a).mint(user, &(1_000_000 * ONE));
        StellarAssetClient::new(&ctx.env, &token_b).mint(user, &(1_000_000 * ONE));
    }

    AmmFixture {
        ctx,
        pool,
        token_a,
        token_b,
        lp_1,
        lp_2,
        trader,
    }
}

fn initialize_pool(fx: &AmmFixture) -> DynamicAmmContractClient<'_> {
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);
    client.initialize(
        &fx.ctx.admin,
        &fx.token_a,
        &fx.token_b,
        &DEFAULT_MIN_FEE_BPS,
        &DEFAULT_MAX_FEE_BPS,
        &DEFAULT_BASE_FEE_BPS,
        &DEFAULT_VOLATILITY_FACTOR,
        &DEFAULT_TWAP_WINDOW,
    );
    client
}

#[test]
fn initialize_pool_sets_configuration_and_initial_reserves() {
    let fx = setup();
    let client = initialize_pool(&fx);

    let config = client.get_config();
    assert_eq!(config.admin, fx.ctx.admin);
    assert_eq!(config.token_a, fx.token_a);
    assert_eq!(config.token_b, fx.token_b);
    assert_eq!(config.min_fee_bps, DEFAULT_MIN_FEE_BPS);
    assert_eq!(config.max_fee_bps, DEFAULT_MAX_FEE_BPS);
    assert_eq!(config.base_fee_bps, DEFAULT_BASE_FEE_BPS);
    assert_eq!(config.volatility_factor, DEFAULT_VOLATILITY_FACTOR);
    assert_eq!(config.twap_window, DEFAULT_TWAP_WINDOW);

    let (res_a, res_b) = client.get_reserves();
    assert_eq!(res_a, 0);
    assert_eq!(res_b, 0);
    assert_eq!(client.get_total_shares(), 0);
}

#[test]
fn initialize_twice_is_rejected() {
    let fx = setup();
    let client = initialize_pool(&fx);

    let err = client.try_initialize(
        &fx.ctx.admin,
        &fx.token_a,
        &fx.token_b,
        &DEFAULT_MIN_FEE_BPS,
        &DEFAULT_MAX_FEE_BPS,
        &DEFAULT_BASE_FEE_BPS,
        &DEFAULT_VOLATILITY_FACTOR,
        &DEFAULT_TWAP_WINDOW,
    );
    assert_eq!(err.unwrap_err().unwrap(), DynamicAmmError::AlreadyInitialized);
}

#[test]
fn invalid_parameters_are_rejected() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    // Identical tokens
    let err = client.try_initialize(
        &fx.ctx.admin,
        &fx.token_a,
        &fx.token_a,
        &10,
        &500,
        &30,
        &2,
        &300,
    );
    assert_eq!(err.unwrap_err().unwrap(), DynamicAmmError::InvalidToken);

    // Min fee > base fee
    let err = client.try_initialize(
        &fx.ctx.admin,
        &fx.token_a,
        &fx.token_b,
        &50,
        &500,
        &30,
        &2,
        &300,
    );
    assert_eq!(err.unwrap_err().unwrap(), DynamicAmmError::InvalidParameter);

    // Base fee > max fee
    let err = client.try_initialize(
        &fx.ctx.admin,
        &fx.token_a,
        &fx.token_b,
        &10,
        &25,
        &30,
        &2,
        &300,
    );
    assert_eq!(err.unwrap_err().unwrap(), DynamicAmmError::InvalidParameter);

    // Zero TWAP window
    let err = client.try_initialize(
        &fx.ctx.admin,
        &fx.token_a,
        &fx.token_b,
        &10,
        &500,
        &30,
        &2,
        &0,
    );
    assert_eq!(err.unwrap_err().unwrap(), DynamicAmmError::InvalidParameter);
}

#[test]
fn initial_liquidity_provisioning_mints_lp_shares() {
    let fx = setup();
    let client = initialize_pool(&fx);

    let deposit_a = 100_000 * ONE;
    let deposit_b = 100_000 * ONE;
    let shares = client.deposit(&fx.lp_1, &deposit_a, &deposit_b, &1);

    // sqrt(100_000 * 100_000 * ONE^2) = 100_000 * ONE
    assert_eq!(shares, 100_000 * ONE);
    assert_eq!(client.get_total_shares(), 100_000 * ONE);
    assert_eq!(client.get_shares(&fx.lp_1), 100_000 * ONE);

    let (res_a, res_b) = client.get_reserves();
    assert_eq!(res_a, deposit_a);
    assert_eq!(res_b, deposit_b);

    // Initial spot price: 1.0 (PRICE_PRECISION)
    assert_eq!(client.get_spot_price(), PRICE_PRECISION);
}

#[test]
fn subsequent_liquidity_provisioning_proportional_shares() {
    let fx = setup();
    let client = initialize_pool(&fx);

    client.deposit(&fx.lp_1, &(100_000 * ONE), &(100_000 * ONE), &1);

    // LP 2 deposits 50% more liquidity
    let shares_2 = client.deposit(&fx.lp_2, &(50_000 * ONE), &(50_000 * ONE), &1);
    assert_eq!(shares_2, 50_000 * ONE);

    assert_eq!(client.get_total_shares(), 150_000 * ONE);
    assert_eq!(client.get_shares(&fx.lp_2), 50_000 * ONE);

    let (res_a, res_b) = client.get_reserves();
    assert_eq!(res_a, 150_000 * ONE);
    assert_eq!(res_b, 150_000 * ONE);
}

#[test]
fn partial_liquidity_withdrawal_preserves_remaining_reserves() {
    let fx = setup();
    let client = initialize_pool(&fx);

    let initial_deposit = 100_000 * ONE;
    let shares = client.deposit(&fx.lp_1, &initial_deposit, &initial_deposit, &1);

    // Withdraw half shares
    let half_shares = shares / 2;
    let (withdrawn_a, withdrawn_b) = client.withdraw(&fx.lp_1, &half_shares, &1, &1);

    assert_eq!(withdrawn_a, 50_000 * ONE);
    assert_eq!(withdrawn_b, 50_000 * ONE);

    let (res_a, res_b) = client.get_reserves();
    assert_eq!(res_a, 50_000 * ONE);
    assert_eq!(res_b, 50_000 * ONE);
    assert_eq!(client.get_total_shares(), half_shares);
    assert_eq!(client.get_shares(&fx.lp_1), half_shares);
}

#[test]
fn complete_liquidity_withdrawal_cleans_up_shares() {
    let fx = setup();
    let client = initialize_pool(&fx);

    let shares = client.deposit(&fx.lp_1, &(40_000 * ONE), &(40_000 * ONE), &1);
    let (withdrawn_a, withdrawn_b) = client.withdraw(&fx.lp_1, &shares, &1, &1);

    assert_eq!(withdrawn_a, 40_000 * ONE);
    assert_eq!(withdrawn_b, 40_000 * ONE);

    let (res_a, res_b) = client.get_reserves();
    assert_eq!(res_a, 0);
    assert_eq!(res_b, 0);
    assert_eq!(client.get_total_shares(), 0);
    assert_eq!(client.get_shares(&fx.lp_1), 0);
}

#[test]
fn twap_price_accumulator_advances_across_ledger_blocks() {
    let fx = setup();
    let client = initialize_pool(&fx);

    // 100k A : 100k B -> Spot = 1.0
    client.deposit(&fx.lp_1, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Advance time by 100 seconds
    let start_time = fx.ctx.env.ledger().timestamp();
    fx.ctx.env.ledger().set_timestamp(start_time + 100);

    // Swap token A for token B, changing reserves: 110k A : 91k B -> Spot price decreases
    client.swap(&fx.trader, &fx.token_a, &(10_000 * ONE), &1);

    let spot_after_swap = client.get_spot_price();
    assert!(spot_after_swap < PRICE_PRECISION);

    // Advance time by another 250s (total elapsed 350s > twap_window 300s)
    fx.ctx.env.ledger().set_timestamp(start_time + 350);
    client.sync_twap();

    let twap = client.get_twap_price();
    // TWAP should lie between initial spot price (1.0) and new spot price (< 1.0)
    assert!(twap <= PRICE_PRECISION);
    assert!(twap >= spot_after_swap);
}

#[test]
fn dynamic_fee_scales_up_during_high_volatility() {
    let fx = setup();
    let client = initialize_pool(&fx);

    client.deposit(&fx.lp_1, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Initially calm pool: dynamic fee is base fee (30 bps)
    assert_eq!(client.get_dynamic_fee_bps(), DEFAULT_BASE_FEE_BPS);

    // Large swap causes severe price deviation
    client.swap(&fx.trader, &fx.token_a, &(40_000 * ONE), &1);

    // Fee should increase significantly beyond base fee
    let dynamic_fee = client.get_dynamic_fee_bps();
    assert!(
        dynamic_fee > DEFAULT_BASE_FEE_BPS,
        "Dynamic fee {} should exceed base fee {}",
        dynamic_fee,
        DEFAULT_BASE_FEE_BPS
    );
    assert!(dynamic_fee <= DEFAULT_MAX_FEE_BPS);
}

#[test]
fn dynamic_fee_cools_down_when_price_stabilizes() {
    let fx = setup();
    let client = initialize_pool(&fx);

    client.deposit(&fx.lp_1, &(100_000 * ONE), &(100_000 * ONE), &1);

    // Large swap creating volatility
    client.swap(&fx.trader, &fx.token_a, &(30_000 * ONE), &1);
    let elevated_fee = client.get_dynamic_fee_bps();
    assert!(elevated_fee > DEFAULT_BASE_FEE_BPS);

    // Time advances past twap window without new disruptive trades
    let current_time = fx.ctx.env.ledger().timestamp();
    fx.ctx.env
        .ledger()
        .set_timestamp(current_time + DEFAULT_TWAP_WINDOW + 50);
    client.sync_twap();

    let cooled_fee = client.get_dynamic_fee_bps();
    // As TWAP catches up to the new reserve ratio, fee returns toward base fee
    assert!(
        cooled_fee <= elevated_fee,
        "Cooled fee {} should be <= elevated fee {}",
        cooled_fee,
        elevated_fee
    );
}
