//! soroban-testkit-assert: events across four contracts, the three-level
//! authorization tree a routed swap needs, and exact error assertions for
//! slippage guards and failures that propagate up from a token.

mod common;

use common::{Amm, ONE};
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
use soroban_sdk::{vec, Address, Env, Error, IntoVal, Symbol, Val};
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{
    assert_auth_matches, assert_auth_sub_invocation, assert_balance_conserved,
    assert_contract_error, assert_event_count_range, assert_event_topic_count, assert_host_error,
};
use soroban_testkit_example_amm::{AmmError, TokenError, DEFAULT_FEE_BPS};

struct Fixture {
    env: Env,
    amm: Amm,
    lp: Address,
    trader: Address,
}

fn setup() -> Fixture {
    let env = Env::default();
    env.mock_all_auths();
    let admin = Address::generate(&env);
    let amm = Amm::deploy(&env, &admin, DEFAULT_FEE_BPS);
    let lp = Address::generate(&env);
    let trader = Address::generate(&env);
    amm.fund(&lp, 1_000_000 * ONE);
    amm.fund(&trader, 1_000_000 * ONE);
    amm.pool()
        .deposit(&lp, &(100_000 * ONE), &(100_000 * ONE), &0, &0);
    Fixture {
        env,
        amm,
        lp,
        trader,
    }
}

// --- events -----------------------------------------------------------------

#[test]
fn a_deposit_emits_one_pool_event_two_transfers_and_a_share_mint() {
    let fx = setup();

    fx.amm
        .pool()
        .deposit(&fx.lp, &(1_000 * ONE), &(1_000 * ONE), &0, &0);

    EventMatcher::new(&fx.env)
        .from_contract(&fx.amm.pool)
        .with_topic("deposit")
        .assert_emitted();
    assert_event_topic_count!(fx.env, "transfer", 2);
    assert_event_topic_count!(fx.env, from = fx.amm.share, "mint", 1);
    assert_event_count_range!(fx.env, from = fx.amm.pool, 1);
}

#[test]
fn a_routed_swap_emits_the_swap_from_the_pool_not_the_router() {
    let fx = setup();

    fx.amm
        .router()
        .swap_exact_in(&fx.amm.pool, &fx.trader, &true, &(1_000 * ONE), &0);

    EventMatcher::new(&fx.env)
        .from_contract(&fx.amm.pool)
        .with_topic("swap")
        .assert_emitted();
    assert_event_count_range!(fx.env, from = fx.amm.router, 0);
    assert_event_topic_count!(fx.env, from = fx.amm.token_a, "transfer", 1);
    assert_event_topic_count!(fx.env, from = fx.amm.token_b, "transfer", 1);
}

#[test]
fn a_withdraw_burns_shares_and_pays_out_both_tokens() {
    let fx = setup();
    let shares = fx.amm.share().balance(&fx.lp);

    fx.amm.pool().withdraw(&fx.lp, &(shares / 4), &0, &0);

    assert_event_topic_count!(fx.env, from = fx.amm.share, "burn", 1);
    assert_event_topic_count!(fx.env, from = fx.amm.pool, "withdraw", 1);
    assert_event_topic_count!(fx.env, "transfer", 2);
    EventMatcher::new(&fx.env)
        .with_topic("mint")
        .assert_not_emitted();
}

#[test]
fn an_event_log_collects_a_trading_session() {
    let fx = setup();
    let mut log = EventLog::new(&fx.env);

    for buy_a in [true, false, true] {
        fx.amm.pool().swap(&fx.trader, &buy_a, &(500 * ONE), &0);
        log.collect();
    }

    log.matcher().with_topic("swap").assert_count(3);
    assert_event_topic_count!(log, "transfer", 6);
}

// --- authorization ----------------------------------------------------------

#[test]
fn a_deposit_is_authorized_by_the_depositor_with_its_exact_arguments() {
    let fx = setup();

    fx.amm
        .pool()
        .deposit(&fx.lp, &(2_000 * ONE), &(2_000 * ONE), &0, &0);

    assert_auth_matches!(fx.env, {
        invoker: fx.lp,
        function: "deposit",
        args: [fx.lp.clone(), 2_000 * ONE, 2_000 * ONE, 0i128, 0i128],
    });
}

#[test]
fn the_traders_authorization_covers_the_token_transfer_beneath_the_pool() {
    let fx = setup();

    fx.amm
        .router()
        .swap_exact_in(&fx.amm.pool, &fx.trader, &false, &(3_000 * ONE), &0);

    assert_auth_matches!(fx.env, {
        invoker: fx.trader,
        function: "swap_exact_in",
    });
    assert_auth_sub_invocation!(fx.env, {
        contract: fx.amm.token_a,
        function: "transfer",
        args: [fx.trader.clone(), fx.amm.pool.clone(), 3_000 * ONE],
    });
}

#[test]
fn burning_shares_needs_the_lp_not_the_pool() {
    let fx = setup();
    let shares = fx.amm.share().balance(&fx.lp);

    fx.amm.pool().withdraw(&fx.lp, &(shares / 2), &0, &0);

    assert_auth_sub_invocation!(fx.env, {
        contract: fx.amm.share,
        function: "burn",
        args: [fx.lp.clone(), shares / 2],
    });
}

/// The whole tree spelled out by hand: one signature from the trader covers
/// the router call, the pool call it makes, and the transfer the pool makes.
#[test]
fn a_hand_written_three_level_mock_auth_tree_is_enough_for_a_routed_swap() {
    let fx = setup();
    let amount = 1_000 * ONE;
    let pool_args: soroban_sdk::Vec<Val> =
        (fx.trader.clone(), true, amount, 0i128).into_val(&fx.env);
    let transfer_args: soroban_sdk::Vec<Val> =
        (fx.trader.clone(), fx.amm.pool.clone(), amount).into_val(&fx.env);

    let out = fx
        .amm
        .router()
        .mock_auths(&[MockAuth {
            address: &fx.trader,
            invoke: &MockAuthInvoke {
                contract: &fx.amm.router,
                fn_name: "swap_exact_in",
                args: (fx.amm.pool.clone(), fx.trader.clone(), true, amount, 0i128)
                    .into_val(&fx.env),
                sub_invokes: &[MockAuthInvoke {
                    contract: &fx.amm.pool,
                    fn_name: "swap",
                    args: pool_args,
                    sub_invokes: &[MockAuthInvoke {
                        contract: &fx.amm.token_b,
                        fn_name: "transfer",
                        args: transfer_args,
                        sub_invokes: &[],
                    }],
                }],
            },
        }])
        .swap_exact_in(&fx.amm.pool, &fx.trader, &true, &amount, &0);

    assert!(out > 0);
}

#[test]
fn a_tree_missing_the_nested_calls_is_refused() {
    let fx = setup();
    let amount = 1_000 * ONE;
    fx.env.mock_auths(&[MockAuth {
        address: &fx.trader,
        invoke: &MockAuthInvoke {
            contract: &fx.amm.router,
            fn_name: "swap_exact_in",
            args: (fx.amm.pool.clone(), fx.trader.clone(), true, amount, 0i128).into_val(&fx.env),
            sub_invokes: &[],
        },
    }]);

    // Through the generic invoker, so the host error keeps its category
    // rather than collapsing into the client's `AmmError` type. The auth
    // check fails inside the token, two native frames down; the host catches
    // that as a failed contract frame, so what reaches the test is
    // context/InvalidAction (the diagnostic log still records the Auth error).
    let outcome = fx.env.try_invoke_contract::<i128, Error>(
        &fx.amm.router,
        &Symbol::new(&fx.env, "swap_exact_in"),
        vec![
            &fx.env,
            fx.amm.pool.into_val(&fx.env),
            fx.trader.into_val(&fx.env),
            true.into_val(&fx.env),
            amount.into_val(&fx.env),
            0i128.into_val(&fx.env),
        ],
    );

    assert_host_error!(outcome, ScErrorType::Context, ScErrorCode::InvalidAction);
}

// --- errors -----------------------------------------------------------------

#[test]
fn a_swap_below_the_minimum_output_is_refused_as_slippage() {
    let fx = setup();
    let quoted = fx.amm.pool().quote(&true, &(1_000 * ONE));

    assert_contract_error!(
        fx.amm
            .pool()
            .try_swap(&fx.trader, &true, &(1_000 * ONE), &(quoted + 1)),
        AmmError::SlippageExceeded
    );
}

#[test]
fn the_router_passes_the_pools_slippage_error_through_unchanged() {
    let fx = setup();

    assert_contract_error!(
        fx.amm.router().try_swap_exact_in(
            &fx.amm.pool,
            &fx.trader,
            &true,
            &(1_000 * ONE),
            &(1_000 * ONE)
        ),
        AmmError::SlippageExceeded
    );
}

#[test]
fn a_deposit_short_of_its_minimums_is_refused_as_slippage() {
    let fx = setup();

    assert_contract_error!(
        fx.amm
            .pool()
            .try_deposit(&fx.lp, &(1_000 * ONE), &(5_000 * ONE), &0, &(5_000 * ONE)),
        AmmError::SlippageExceeded
    );
}

#[test]
fn configuration_errors_name_their_variant() {
    let fx = setup();

    assert_contract_error!(
        fx.amm
            .pool()
            .try_initialize(&fx.amm.token_a, &fx.amm.token_b, &fx.amm.share, &30),
        AmmError::AlreadyInitialized
    );
    assert_contract_error!(
        fx.amm.pool().try_swap(&fx.trader, &true, &0, &0),
        AmmError::InvalidAmount
    );
}

#[test]
fn swapping_against_an_empty_pool_is_insufficient_liquidity() {
    let env = Env::default();
    env.mock_all_auths();
    let amm = Amm::deploy(&env, &Address::generate(&env), DEFAULT_FEE_BPS);
    let trader = Address::generate(&env);

    assert_contract_error!(
        amm.pool().try_swap(&trader, &true, &ONE, &0),
        AmmError::InsufficientLiquidity
    );
}

/// The trader has no B, so the token refuses three frames down. The code is a
/// `TokenError`, which the pool's `AmmError` client cannot name — so it is
/// asserted as a raw contract error.
#[test]
fn a_token_failure_beneath_the_pool_surfaces_with_the_tokens_code() {
    let fx = setup();
    let broke = Address::generate(&fx.env);

    let outcome = fx.amm.pool().try_swap(&broke, &true, &ONE, &0);

    assert_host_error!(
        outcome,
        Error::from_contract_error(TokenError::InsufficientBalance as u32)
    );
}

#[test]
#[should_panic(expected = "received: SlippageExceeded (code 4, category contract)")]
fn a_wrong_expectation_prints_both_variants() {
    let fx = setup();

    assert_contract_error!(
        fx.amm
            .pool()
            .try_swap(&fx.trader, &true, &(1_000 * ONE), &(1_000 * ONE)),
        AmmError::InsufficientLiquidity
    );
}

#[test]
fn a_swap_conserves_tokens_between_trader_and_pool() {
    let fx = setup();
    let token_a = fx.amm.token_a();

    assert_balance_conserved!(
        accounts: [(&fx.trader, "trader"), (&fx.amm.pool, "pool")],
        balance: |addr| token_a.balance(addr),
        call: {
            fx.amm.pool().swap(&fx.trader, &true, &(2_000 * ONE), &0);
        }
    );
}
