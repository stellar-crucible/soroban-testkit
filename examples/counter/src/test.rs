extern crate std;

use soroban_sdk::{Address, Env};
use testkit_assert::events::EventMatcher;
use testkit_core::budget::BudgetSnapshot;
use testkit_fixtures::builder::TestContextBuilder;
use testkit_fixtures::TestContext;

use crate::{CounterContract, CounterContractClient};

fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

fn client_for(ctx: &TestContext) -> (Address, CounterContractClient<'_>) {
    let contract_id = ctx.env.register(CounterContract, ());
    (
        contract_id.clone(),
        CounterContractClient::new(&ctx.env, &contract_id),
    )
}

#[test]
fn counter_starts_at_zero() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);

    assert_eq!(client.get(), 0);
}

#[test]
fn increment_accumulates_and_returns_the_new_value() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller: Address = ctx.users[0].clone();

    assert_eq!(client.increment(&caller, &5), 5);
    assert_eq!(client.increment(&caller, &7), 12);
    assert_eq!(client.get(), 12);
}

#[test]
fn increment_emits_exactly_one_event() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);

    EventMatcher::new(&ctx.env).assert_count(1);
    EventMatcher::new(&ctx.env).assert_emitted();
}

#[test]
fn read_only_calls_emit_nothing() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);

    client.get();

    EventMatcher::new(&ctx.env).assert_not_emitted();
}

#[test]
fn a_topic_the_contract_never_publishes_stays_silent() {
    let ctx = context(1);
    let (contract_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);

    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("decremented")
        .assert_not_emitted();
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("incremented")
        .assert_emitted();
}

#[test]
fn each_invocation_is_asserted_on_its_own() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);
    EventMatcher::new(&ctx.env).assert_count(1);

    client.increment(&caller, &2);
    EventMatcher::new(&ctx.env).assert_count(1);
}

#[test]
fn events_can_be_filtered_to_the_contract_that_emitted_them() {
    let ctx = context(1);
    let (contract_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);

    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("counter")
        .assert_emitted();
}

#[test]
fn increment_consumes_a_predictable_amount_of_cpu() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    // Warm up the cost estimator so a budget snapshot is available.
    client.get();

    let env: &Env = &ctx.env;
    let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
    client.increment(&caller, &1);
    let after = BudgetSnapshot::capture(&env.cost_estimate().budget());

    let cost = before.diff(&after);
    assert!(
        cost.cpu_insns > 0,
        "a contract call must consume CPU instructions"
    );
    assert!(
        cost.cpu_insns < 20_000_000,
        "counter increment regressed to {} CPU instructions",
        cost.cpu_insns
    );
}

#[test]
#[should_panic]
fn increment_requires_authorization_when_auths_are_not_mocked() {
    let ctx = TestContextBuilder::new()
        .with_users(1)
        .without_mock_auths()
        .build();
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.increment(&caller, &1);
}
