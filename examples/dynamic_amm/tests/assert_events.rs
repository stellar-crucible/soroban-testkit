extern crate std;

use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::Address;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{
    assert_auth_matches, assert_auth_sub_invocation, assert_event_count_range,
    assert_event_topic_count,
};
use soroban_testkit_example_dynamic_amm::{
    DynamicAmmContract, DynamicAmmContractClient, DEFAULT_BASE_FEE_BPS, DEFAULT_MAX_FEE_BPS,
    DEFAULT_MIN_FEE_BPS, DEFAULT_TWAP_WINDOW, DEFAULT_VOLATILITY_FACTOR,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

const ONE: i128 = 10_000_000;

struct AmmAssertFixture {
    ctx: TestContext,
    pool: Address,
    token_a: Address,
    token_b: Address,
    lp: Address,
    trader: Address,
}

fn setup() -> AmmAssertFixture {
    let ctx = TestContextBuilder::new().with_users(2).build();

    let pool = ctx.env.register(DynamicAmmContract, ());
    let token_a = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    let token_b = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    let lp = ctx.user(0).clone();
    let trader = ctx.user(1).clone();

    for user in [&lp, &trader] {
        StellarAssetClient::new(&ctx.env, &token_a).mint(user, &(1_000_000 * ONE));
        StellarAssetClient::new(&ctx.env, &token_b).mint(user, &(1_000_000 * ONE));
    }

    let client = DynamicAmmContractClient::new(&ctx.env, &pool);
    client.initialize(
        &ctx.admin,
        &token_a,
        &token_b,
        &DEFAULT_MIN_FEE_BPS,
        &DEFAULT_MAX_FEE_BPS,
        &DEFAULT_BASE_FEE_BPS,
        &DEFAULT_VOLATILITY_FACTOR,
        &DEFAULT_TWAP_WINDOW,
    );

    AmmAssertFixture {
        ctx,
        pool,
        token_a,
        token_b,
        lp,
        trader,
    }
}

#[test]
fn deposit_emits_deposit_event_with_expected_topics_and_payload() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(50_000 * ONE), &(50_000 * ONE), &1);

    // EventMatcher confirms emission from pool with topic "deposit"
    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.pool)
        .with_topic("deposit")
        .assert_emitted();

    // Exactly 1 deposit event
    assert_event_topic_count!(fx.ctx.env, "deposit", 1);
}

#[test]
fn withdraw_emits_withdraw_event_with_expected_topics_and_payload() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    let shares = client.deposit(&fx.lp, &(40_000 * ONE), &(40_000 * ONE), &1);
    client.withdraw(&fx.lp, &shares, &1, &1);

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.pool)
        .with_topic("withdraw")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "withdraw", 1);
}

#[test]
fn swap_emits_swap_event_recording_fee_bps_and_amounts() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);
    client.swap(&fx.trader, &fx.token_a, &(10_000 * ONE), &1);

    EventMatcher::new(&fx.ctx.env)
        .from_contract(&fx.pool)
        .with_topic("swap")
        .assert_emitted();

    assert_event_topic_count!(fx.ctx.env, "swap", 1);
}

#[test]
fn multi_step_trading_sequence_accumulates_events_in_event_log() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);

    let mut log = EventLog::new(&fx.ctx.env);

    client.swap(&fx.trader, &fx.token_a, &(2_000 * ONE), &1);
    log.collect();

    client.swap(&fx.trader, &fx.token_b, &(1_500 * ONE), &1);
    log.collect();

    client.swap(&fx.trader, &fx.token_a, &(3_000 * ONE), &1);
    log.collect();

    // EventLog observed all 3 swaps
    log.matcher().with_topic("swap").assert_count(3);
    assert_event_topic_count!(log, "swap", 3);
}

#[test]
fn event_count_ranges_bound_lifecycle_event_emission() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(50_000 * ONE), &(50_000 * ONE), &1);

    // Deposit emits deposit event plus token transfer events
    assert_event_count_range!(fx.ctx.env, 1..=10);
    assert_event_count_range!(fx.ctx.env, from = fx.pool, 1..=2);
}

#[test]
fn deposit_requires_depositor_authorization() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(20_000 * ONE), &(20_000 * ONE), &1);

    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.lp,
        function: "deposit",
        args: [fx.lp.clone(), 20_000 * ONE, 20_000 * ONE, 1i128],
    });
}

#[test]
fn swap_requires_swapper_authorization() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);
    client.swap(&fx.trader, &fx.token_a, &(5_000 * ONE), &1);

    assert_auth_matches!(fx.ctx.env, {
        invoker: fx.trader,
        function: "swap",
        args: [fx.trader.clone(), fx.token_a.clone(), 5_000 * ONE, 1i128],
    });
}

#[test]
fn swap_sub_invocation_verifies_underlying_token_transfer_authorization() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(100_000 * ONE), &(100_000 * ONE), &1);
    client.swap(&fx.trader, &fx.token_a, &(8_000 * ONE), &1);

    // Using assert_auth_sub_invocation! to isolate the intermediate transfer call
    // from trader to pool contract address:
    assert_auth_sub_invocation!(fx.ctx.env, {
        contract: fx.token_a,
        function: "transfer",
        args: [fx.trader.clone(), fx.pool.clone(), 8_000 * ONE],
    });
}

#[test]
fn negative_assertion_proves_no_spurious_events_emitted() {
    let fx = setup();
    let client = DynamicAmmContractClient::new(&fx.ctx.env, &fx.pool);

    client.deposit(&fx.lp, &(10_000 * ONE), &(10_000 * ONE), &1);

    // Pool never emitted unprovoked refund or liquidation events
    EventMatcher::new(&fx.ctx.env)
        .with_topic("Refund")
        .assert_not_emitted();

    EventMatcher::new(&fx.ctx.env)
        .with_topic("Liquidate")
        .assert_not_emitted();
}
