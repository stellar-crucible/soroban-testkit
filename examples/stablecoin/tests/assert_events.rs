// One crate per file: this suite drives `examples/stablecoin` through
// `soroban-testkit-assert`, verifying multi-contract events, event payloads,
// invocation sequences via `EventLog`, single-hop authorizations via
// `assert_auth_matches!`, and multi-hop router authorizations via
// `assert_auth_sub_tree_matches!`.
use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
use soroban_sdk::{Address, IntoVal, Symbol};
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{assert_auth_matches, assert_auth_sub_tree_matches};
use soroban_testkit_example_stablecoin::{
    PriceOracleContract, PriceOracleContractClient, StablecoinContract, StablecoinContractClient,
    VaultRouterContract, VaultRouterContractClient, DEFAULT_MCR_BPS, PRICE_SCALE,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::TestContext;

struct Deployed<'a> {
    oracle_id: Address,
    oracle: PriceOracleContractClient<'a>,
    stable_id: Address,
    stable: StablecoinContractClient<'a>,
    router_id: Address,
    router: VaultRouterContractClient<'a>,
}

fn deploy(ctx: &TestContext) -> Deployed<'_> {
    let oracle_id = ctx.env.register(PriceOracleContract, ());
    let oracle = PriceOracleContractClient::new(&ctx.env, &oracle_id);
    oracle.initialize(&ctx.admin);

    let stable_id = ctx.env.register(StablecoinContract, ());
    let stable = StablecoinContractClient::new(&ctx.env, &stable_id);
    stable.initialize(&ctx.admin, &oracle_id, &DEFAULT_MCR_BPS);

    let router_id = ctx.env.register(VaultRouterContract, ());
    let router = VaultRouterContractClient::new(&ctx.env, &router_id);

    let xlm = Symbol::new(&ctx.env, "XLM");
    let wbtc = Symbol::new(&ctx.env, "WBTC");
    oracle.set_price(&xlm, &PRICE_SCALE);
    oracle.set_price(&wbtc, &(50_000 * PRICE_SCALE));
    stable.register_collateral(&xlm, &15_000);
    stable.register_collateral(&wbtc, &16_000);

    Deployed {
        oracle_id,
        oracle,
        stable_id,
        stable,
        router_id,
        router,
    }
}

#[test]
fn deposit_event_carries_owner_asset_amount_and_total() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.stable.deposit_collateral(&borrower, &xlm, &2_500);

    EventMatcher::new(&ctx.env)
        .from_contract(&d.stable_id)
        .with_topic("deposit")
        .assert_data_matches(|data| {
            data.field("owner")
                .and_then(|value| value.deserialize::<Address>())
                == Some(borrower.clone())
                && data
                    .field("amount")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(2_500)
                && data
                    .field("total_collateral")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(2_500)
        });
}

#[test]
fn mint_event_fields_pin_the_exact_payload_shape() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.stable.deposit_collateral(&borrower, &xlm, &1_500);
    d.stable.mint(&borrower, &xlm, &1_000);

    EventMatcher::new(&ctx.env)
        .from_contract(&d.stable_id)
        .with_topic("mint")
        .assert_data_matches(|data| {
            let mut names = data.fields();
            names.sort();
            names == ["amount", "asset", "owner", "total_debt"]
        });
}

#[test]
fn oracle_and_stablecoin_keep_their_events_separate_with_from_contract() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.oracle.set_price(&xlm, &(2 * PRICE_SCALE));

    EventMatcher::new(&ctx.env)
        .from_contract(&d.oracle_id)
        .with_topic("price")
        .assert_emitted();
    EventMatcher::new(&ctx.env)
        .from_contract(&d.stable_id)
        .with_topic("price")
        .assert_not_emitted();
}

#[test]
fn read_only_queries_emit_no_events() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.stable.total_supply();
    d.stable.balance_of(ctx.user(0));
    d.stable.vault(ctx.user(0), &xlm);

    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn full_vault_lifecycle_events_collect_in_order_with_event_log() {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let staker = ctx.user(1).clone();
    let liquidator = ctx.user(2).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");
    let wbtc = Symbol::new(&ctx.env, "WBTC");

    // Seed the Stability Pool before starting the logged lifecycle.
    d.stable.deposit_collateral(&staker, &wbtc, &1);
    d.stable.mint(&staker, &wbtc, &1_500);

    let mut log = EventLog::new(&ctx.env);
    d.stable.deposit_collateral(&borrower, &xlm, &1_500);
    log.collect();
    d.stable.mint(&borrower, &xlm, &1_000);
    log.collect();
    d.stable.stake_stability_pool(&staker, &1_200);
    log.collect();
    d.oracle.set_price(&xlm, &(8 * PRICE_SCALE / 10));
    log.collect();
    d.stable.liquidate(&liquidator, &borrower, &xlm);
    log.collect();

    log.matcher()
        .from_contract(&d.stable_id)
        .with_topic("liquidate")
        .assert_count(1);
    log.matcher()
        .from_contract(&d.oracle_id)
        .with_topic("price")
        .assert_count(1);
    log.matcher().with_topic("redeem").assert_not_emitted();

    let topics: Vec<String> = log.topics().iter().map(|t| t[1].clone()).collect();
    assert_eq!(
        topics,
        ["deposit", "mint", "sp_stake", "price", "liquidate"]
    );
}

#[test]
fn redemption_event_records_algorithmic_fee_bps_and_collateral_out() {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let redeemer = ctx.user(1).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.stable.deposit_collateral(&borrower, &xlm, &2_000);
    d.stable.mint(&borrower, &xlm, &1_000);
    d.stable.deposit_collateral(&redeemer, &xlm, &2_000);
    d.stable.mint(&redeemer, &xlm, &1_000);

    d.stable.redeem(&redeemer, &borrower, &xlm, &100);

    EventMatcher::new(&ctx.env)
        .from_contract(&d.stable_id)
        .with_topic("redeem")
        .assert_data_matches(|data| {
            data.field("fee_bps")
                .and_then(|value| value.deserialize::<i128>())
                == Some(300)
                && data
                    .field("collateral_out")
                    .and_then(|value| value.deserialize::<i128>())
                    == Some(97)
        });
}

#[test]
fn mint_records_borrower_authorization_with_arguments() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.stable.deposit_collateral(&borrower, &xlm, &1_500);
    d.stable.mint(&borrower, &xlm, &1_000);

    assert_auth_matches!(ctx.env, {
        invoker: borrower,
        function: "mint",
        args: [_, xlm, 1_000i128],
    });
}

#[test]
fn router_deposit_and_mint_validates_nested_sub_tree_authorizations() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.router
        .deposit_and_mint(&d.stable_id, &borrower, &xlm, &1_500, &1_000);

    // Validate the full router -> [deposit_collateral, mint] authorization tree.
    assert_auth_sub_tree_matches!(ctx.env, {
        invoker: borrower,
        contract: d.router_id,
        function: "deposit_and_mint",
        sub_invocations: [
            {
                contract: d.stable_id,
                function: "deposit_collateral",
                args: [_, xlm.clone(), 1_500i128],
                sub_invocations: [],
            },
            {
                contract: d.stable_id,
                function: "mint",
                args: [_, xlm.clone(), 1_000i128],
                sub_invocations: [],
            },
        ],
    });

    // Also match only the nested `mint` leaf directly without mentioning the router.
    assert_auth_sub_tree_matches!(ctx.env, {
        invoker: borrower,
        contract: d.stable_id,
        function: "mint",
        args: [borrower.clone(), xlm, 1_000i128],
    });
}

#[test]
fn auth_matcher_counts_invocation_requirements_and_reads_none() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let d = deploy(&ctx);
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    d.stable.deposit_collateral(&borrower, &xlm, &1_500);
    AuthMatcher::new(&ctx.env).assert_auth_count(1);

    d.stable.vault(&borrower, &xlm);
    AuthMatcher::new(&ctx.env).assert_no_auth_required();
}

#[test]
fn explicit_mock_auth_authorizes_collateral_deposit() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let oracle_id = ctx.env.register(PriceOracleContract, ());
    let oracle = PriceOracleContractClient::new(&ctx.env, &oracle_id);
    let stable_id = ctx.env.register(StablecoinContract, ());
    let stable = StablecoinContractClient::new(&ctx.env, &stable_id);
    let admin = ctx.admin.clone();
    let borrower = ctx.user(0).clone();
    let xlm = Symbol::new(&ctx.env, "XLM");

    ctx.env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &oracle_id,
            fn_name: "initialize",
            args: (admin.clone(),).into_val(&ctx.env),
            sub_invokes: &[],
        },
    }]);
    oracle.initialize(&admin);

    ctx.env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &stable_id,
            fn_name: "initialize",
            args: (admin.clone(), oracle_id.clone(), DEFAULT_MCR_BPS).into_val(&ctx.env),
            sub_invokes: &[],
        },
    }]);
    stable.initialize(&admin, &oracle_id, &DEFAULT_MCR_BPS);

    ctx.env.mock_auths(&[MockAuth {
        address: &admin,
        invoke: &MockAuthInvoke {
            contract: &stable_id,
            fn_name: "register_collateral",
            args: (xlm.clone(), 15_000i128).into_val(&ctx.env),
            sub_invokes: &[],
        },
    }]);
    stable.register_collateral(&xlm, &15_000);

    ctx.env.mock_auths(&[MockAuth {
        address: &borrower,
        invoke: &MockAuthInvoke {
            contract: &stable_id,
            fn_name: "deposit_collateral",
            args: (borrower.clone(), xlm.clone(), 1_500i128).into_val(&ctx.env),
            sub_invokes: &[],
        },
    }]);
    stable.deposit_collateral(&borrower, &xlm, &1_500);

    assert_auth_matches!(ctx.env, {
        invoker: borrower,
        function: "deposit_collateral",
        args: [_, xlm, 1_500i128],
    });
}

#[test]
#[should_panic]
fn unauthorized_mint_is_refused() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let stable_id = ctx.env.register(StablecoinContract, ());
    let stable = StablecoinContractClient::new(&ctx.env, &stable_id);
    let xlm = Symbol::new(&ctx.env, "XLM");

    stable.mint(ctx.user(0), &xlm, &100);
}
