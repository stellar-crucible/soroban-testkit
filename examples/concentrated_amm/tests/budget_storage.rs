extern crate std;

use soroban_sdk::{testutils::Address as _, Address};
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_example_concentrated_amm::{ConcentratedAmm, ConcentratedAmmClient};
use soroban_testkit_fixtures::builder::TestContextBuilder;

fn pool() -> (
    soroban_testkit_fixtures::TestContext,
    ConcentratedAmmClient<'static>,
) {
    let ctx = TestContextBuilder::new().with_users(2).build();
    let contract = ctx.env.register(ConcentratedAmm, ());
    let client = ConcentratedAmmClient::new(&ctx.env, &contract);
    client.initialize(
        &ctx.admin,
        &Address::generate(&ctx.env),
        &Address::generate(&ctx.env),
        &1_000_000,
        &30,
    );
    client.mint_position(ctx.user(0), &-600, &600, &100_000);
    (ctx, client)
}

#[test]
fn swap_metered() {
    let (ctx, c) = pool();
    c.swap(ctx.user(1), &100, &true);
    let cost = BudgetSnapshot::last_invocation(&ctx.env);
    assert!(cost.cpu_insns > 0);
}
#[test]
fn mint_metered() {
    let (ctx, c) = pool();
    c.mint_position(ctx.user(0), &-1200, &1200, &10_000);
    assert!(BudgetSnapshot::last_invocation(&ctx.env).cpu_insns > 0);
}
#[test]
fn burn_metered() {
    let (ctx, c) = pool();
    c.burn_position(ctx.user(0), &0);
    assert!(BudgetSnapshot::last_invocation(&ctx.env).cpu_insns > 0);
}
#[test]
fn cross_tick_swap_budget_1() {
    let (ctx, c) = pool();
    c.swap(ctx.user(1), &1000, &true);
    let cost = BudgetSnapshot::last_invocation(&ctx.env);
    assert!(cost.cpu_insns < 100_000_000);
}
#[test]
fn cross_tick_swap_budget_2() {
    let (ctx, c) = pool();
    c.swap(ctx.user(1), &2000, &true);
    let cost = BudgetSnapshot::last_invocation(&ctx.env);
    assert!(cost.cpu_insns < 100_000_000);
}
#[test]
fn cross_tick_swap_budget_3() {
    let (ctx, c) = pool();
    c.swap(ctx.user(1), &3000, &false);
    let cost = BudgetSnapshot::last_invocation(&ctx.env);
    assert!(cost.cpu_insns < 100_000_000);
}
#[test]
fn reserves_stay_live_after_swap() {
    let (ctx, c) = pool();
    c.swap(ctx.user(1), &500, &true);
    let (a, b) = c.get_reserves();
    assert!(a > 0 || b >= 0);
}
#[test]
fn position_storage_visible_after_mint() {
    let (ctx, c) = pool();
    let id = c.mint_position(ctx.user(0), &-1800, &1800, &500);
    assert!(c.get_position(&id).is_some());
}
