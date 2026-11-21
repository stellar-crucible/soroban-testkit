//! soroban-testkit-fixtures: multi-contract setup with `TestContextBuilder`,
//! named users, ledger movement, and the TTL helpers on LP balances.

mod common;

use common::{Amm, ONE};
use soroban_testkit_core::StorageTier;
use soroban_testkit_example_amm::{
    integer_sqrt, quote_amount_out, PoolKey, TokenKey, DEFAULT_FEE_BPS, MINIMUM_LIQUIDITY,
};
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::{TestContext, TtlStatus};

struct Fixture {
    ctx: TestContext,
    amm: Amm,
}

/// Admin plus three users: `lp` (0), `trader` (1) and `second_lp` (2), each
/// funded with a million of both assets.
fn setup() -> Fixture {
    let ctx = TestContextBuilder::new().with_users(3).build();
    let amm = Amm::deploy(&ctx.env, &ctx.admin, DEFAULT_FEE_BPS);
    for user in &ctx.users {
        amm.fund(user, 1_000_000 * ONE);
    }
    Fixture { ctx, amm }
}

fn seeded() -> Fixture {
    let fx = setup();
    fx.amm
        .pool()
        .deposit(fx.ctx.user(0), &(100_000 * ONE), &(100_000 * ONE), &0, &0);
    fx
}

#[test]
fn every_contract_is_registered_in_the_same_environment() {
    let fx = setup();
    let ids = [
        &fx.amm.token_a,
        &fx.amm.token_b,
        &fx.amm.share,
        &fx.amm.pool,
        &fx.amm.router,
    ];

    for (i, a) in ids.iter().enumerate() {
        for b in &ids[i + 1..] {
            assert_ne!(a, b, "every deployment gets its own address");
        }
    }
    assert_eq!(
        fx.amm.share().admin(),
        fx.amm.pool,
        "the pool owns the share token"
    );
    assert_eq!(fx.amm.pool().share_token_id(), fx.amm.share);
    assert_eq!(fx.amm.token_a().balance(fx.ctx.user(1)), 1_000_000 * ONE);
}

#[test]
fn first_deposit_mints_sqrt_of_the_product_minus_locked_liquidity() {
    let fx = setup();
    let lp = fx.ctx.user(0);

    let minted = fx
        .amm
        .pool()
        .deposit(lp, &(40_000 * ONE), &(10_000 * ONE), &0, &0);

    let expected = integer_sqrt(40_000 * ONE * 10_000 * ONE);
    assert_eq!(minted, expected - MINIMUM_LIQUIDITY);
    assert_eq!(fx.amm.share().balance(lp), minted);
    assert_eq!(fx.amm.pool().total_shares(), expected);
    assert_eq!(fx.amm.pool().get_reserves(), (40_000 * ONE, 10_000 * ONE));
}

#[test]
fn a_later_deposit_mints_shares_in_proportion() {
    let fx = seeded();
    let total_before = fx.amm.pool().total_shares();

    let minted = fx
        .amm
        .pool()
        .deposit(fx.ctx.user(2), &(10_000 * ONE), &(10_000 * ONE), &0, &0);

    assert_eq!(
        minted,
        total_before / 10,
        "10% more liquidity, 10% more shares"
    );
}

#[test]
fn a_lopsided_deposit_only_takes_the_pool_ratio() {
    let fx = seeded();
    let second = fx.ctx.user(2);
    let before_b = fx.amm.token_b().balance(second);

    fx.amm
        .pool()
        .deposit(second, &(10_000 * ONE), &(50_000 * ONE), &0, &0);

    assert_eq!(
        before_b - fx.amm.token_b().balance(second),
        10_000 * ONE,
        "only the matching amount of B leaves the wallet"
    );
}

#[test]
fn a_swap_pays_the_quoted_amount_and_moves_both_reserves() {
    let fx = seeded();
    let trader = fx.ctx.user(1);
    let quoted = fx.amm.pool().quote(&true, &(1_000 * ONE));

    let out = fx.amm.pool().swap(trader, &true, &(1_000 * ONE), &0);

    assert_eq!(out, quoted);
    assert_eq!(
        out,
        quote_amount_out(100_000 * ONE, 100_000 * ONE, 1_000 * ONE, DEFAULT_FEE_BPS).unwrap()
    );
    assert_eq!(
        fx.amm.pool().get_reserves(),
        (100_000 * ONE - out, 101_000 * ONE)
    );
    assert_eq!(fx.amm.token_a().balance(trader), 1_000_000 * ONE + out);
}

#[test]
fn the_router_swaps_exactly_like_a_direct_call() {
    let fx = seeded();
    let trader = fx.ctx.user(1);
    let quoted = fx.amm.pool().quote(&false, &(2_500 * ONE));

    let out = fx
        .amm
        .router()
        .swap_exact_in(&fx.amm.pool, trader, &false, &(2_500 * ONE), &quoted);

    assert_eq!(out, quoted);
    assert_eq!(fx.amm.token_b().balance(trader), 1_000_000 * ONE + out);
}

#[test]
fn withdraw_returns_the_pro_rata_share_of_each_reserve() {
    let fx = seeded();
    let lp = fx.ctx.user(0);
    let shares = fx.amm.share().balance(lp);
    let total = fx.amm.pool().total_shares();

    let (a, b) = fx.amm.pool().withdraw(lp, &(shares / 2), &0, &0);

    assert_eq!(a, (shares / 2) * 100_000 * ONE / total);
    assert_eq!(b, a, "a balanced pool pays out balanced");
    assert_eq!(fx.amm.share().balance(lp), shares - shares / 2);
}

#[test]
fn swap_fees_accrue_to_liquidity_providers() {
    let fx = seeded();
    let trader = fx.ctx.user(1);
    let k_before = fx.amm.k();

    for _ in 0..10 {
        fx.amm.pool().swap(trader, &true, &(5_000 * ONE), &0);
        fx.amm.pool().swap(trader, &false, &(5_000 * ONE), &0);
    }

    assert!(fx.amm.k() > k_before, "fees stay in the pool, so k grows");
    let lp = fx.ctx.user(0);
    let shares = fx.amm.share().balance(lp);
    let (a, b) = fx.amm.pool().withdraw(lp, &shares, &0, &0);
    let locked = MINIMUM_LIQUIDITY;
    let deposited_value = 2 * 100_000 * ONE * shares / (shares + locked);
    assert!(
        a + b > deposited_value,
        "the LP leaves with more than it put in: {} > {}",
        a + b,
        deposited_value
    );
}

#[test]
fn users_trade_in_sequence_across_ledgers() {
    let mut fx = seeded();
    let start = fx.ctx.ledger_sequence();

    for round in 0..3u32 {
        fx.ctx.advance_ledger(10);
        fx.ctx.advance_time(50);
        let user = fx.ctx.user(1 + (round as usize % 2)).clone();
        fx.amm
            .pool()
            .swap(&user, &(round % 2 == 0), &(1_000 * ONE), &0);
    }

    assert_eq!(fx.ctx.ledger_sequence(), start + 30);
    let (a, b) = fx.amm.pool().get_reserves();
    assert!(a > 0 && b > 0);
}

#[test]
fn pool_state_lives_in_instance_storage_and_shares_its_ttl() {
    let fx = seeded();

    let reserves = fx.ctx.ttl(&fx.amm.pool, &PoolKey::ReserveA);
    let supply = fx.ctx.ttl(&fx.amm.pool, &PoolKey::TotalShares);

    assert_eq!(reserves.tier, StorageTier::Instance);
    assert_eq!(reserves.live_until_ledger, supply.live_until_ledger);
    assert_eq!(reserves.status(), TtlStatus::Live);
}

#[test]
fn an_lp_balance_left_unattended_is_archived() {
    let mut fx = seeded();
    let lp = fx.ctx.user(0).clone();
    let key = TokenKey::Balance(lp.clone());

    let ttl = fx.ctx.ttl(&fx.amm.share, &key);
    assert_eq!(ttl.tier, StorageTier::Persistent);
    assert_eq!(ttl.min_ttl(), ttl.thresholds.min_persistent_entry_ttl);

    fx.ctx.fast_forward_to_last_live_ledger(&fx.amm.share, &key);
    assert_eq!(fx.ctx.ttl(&fx.amm.share, &key).status(), TtlStatus::Live);

    fx.ctx.fast_forward_to_expiration(&fx.amm.share, &key);
    fx.ctx.assert_persistent_archived(&fx.amm.share, &key);

    let swept = fx.ctx.archive_expired(0);
    assert!(!swept.is_empty());
    assert_eq!(
        fx.amm.share().balance(&lp),
        0,
        "an archived balance reads as nothing until it is restored"
    );
}

#[test]
fn reset_gives_the_same_users_a_fresh_chain() {
    let mut fx = seeded();
    let lp = fx.ctx.user(0).clone();

    fx.ctx.reset();
    let amm = Amm::deploy(&fx.ctx.env, &fx.ctx.admin, DEFAULT_FEE_BPS);

    assert_eq!(fx.ctx.user(0), &lp, "identities survive a reset");
    assert_eq!(
        amm.pool().get_reserves(),
        (0, 0),
        "the new pool starts empty"
    );
    assert_eq!(amm.token_a().balance(&lp), 0);
}
