# Worked Example: Counter

A complete contract and its test suite, line by line. Everything here is the real code in [`examples/counter`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/counter), which is a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/counter</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">8 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p testkit-example-counter</span></div>
</div>

## The contract

A counter that stores one `u32` in instance storage, charges an authorisation, and emits one event per increment.

```rust
#[contract]
pub struct CounterContract;

#[contractevent(topics = ["counter", "incremented"])]
#[derive(Clone)]
pub struct Incremented {
    pub caller: Address,
    pub new_count: u32,
}

#[contractimpl]
impl CounterContract {
    pub fn get(env: Env) -> u32 {
        let key = Symbol::new(&env, "count");
        env.storage().instance().get(&key).unwrap_or(0)
    }

    pub fn increment(env: Env, caller: Address, by: u32) -> u32 {
        caller.require_auth();

        let key = Symbol::new(&env, "count");
        let current: u32 = env.storage().instance().get(&key).unwrap_or(0);
        let next = current.checked_add(by).expect("counter overflow");
        env.storage().instance().set(&key, &next);

        Incremented { caller, new_count: next }.publish(&env);

        next
    }
}
```

## Step 1 — build the environment

`TestContextBuilder` hands back an `Env`, an admin and pre-generated user addresses. Nothing else is set up by hand in this suite.

```rust
use testkit_fixtures::builder::TestContextBuilder;
use testkit_fixtures::TestContext;

fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}
```

Auth is mocked by default, so `caller.require_auth()` passes without wiring a signature per test.

## Step 2 — register and call

Registering returns the contract's `Address`; the generated client borrows the environment. Two helpers keep the borrow checker happy, because a client cannot hold a reference to the context it is stored next to.

```rust
use soroban_sdk::Address;

fn client_for(ctx: &TestContext) -> (Address, CounterContractClient<'_>) {
    let contract_id = ctx.env.register(CounterContract, ());
    (
        contract_id.clone(),
        CounterContractClient::new(&ctx.env, &contract_id),
    )
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
```

## Step 3 — assert on events

`EventMatcher` reads the events of the **most recent invocation**, which is what `env.events().all()` exposes in SDK v28. Assert between calls, not after the whole test.

```rust
use testkit_assert::events::EventMatcher;

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
```

A read-only call emits nothing, which is worth its own test:

```rust
#[test]
fn read_only_calls_emit_nothing() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);

    client.get();

    EventMatcher::new(&ctx.env).assert_count(0);
}
```

## Step 4 — bound the cost

Take a snapshot before and after the call, then assert the delta is non-zero and under a ceiling. The first `get()` warms the cost estimator so the reading is not zero.

```rust
use testkit_core::budget::BudgetSnapshot;

#[test]
fn increment_consumes_a_predictable_amount_of_cpu() {
    let ctx = context(1);
    let (_id, client) = client_for(&ctx);
    let caller = ctx.users[0].clone();

    client.get(); // warm up the estimator

    let env = &ctx.env;
    let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
    client.increment(&caller, &1);
    let after = BudgetSnapshot::capture(&env.cost_estimate().budget());

    let cost = before.diff(&after);
    assert!(cost.cpu_insns > 0);
    assert!(cost.cpu_insns < 20_000_000, "regressed to {} insns", cost.cpu_insns);
}
```

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">Why a ceiling and a floor</span>
  <p><code>cpu_insns &gt; 0</code> catches a call that silently did nothing; the ceiling catches a change that made the call expensive. Together they are a regression test, and they fail with the number that broke the bound.</p>
</div>

## Step 5 — prove the authorisation is real

Everything above mocks auth. The last test turns mocking off and expects the call to panic, which is the only proof that `require_auth()` was not decorative.

```rust
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
```

## Running it

```bash
cargo test -p testkit-example-counter
```

```text
running 8 tests
test test::counter_starts_at_zero ... ok
test test::increment_accumulates_and_returns_the_new_value ... ok
test test::increment_emits_exactly_one_event ... ok
test test::read_only_calls_emit_nothing ... ok
test test::each_invocation_is_asserted_on_its_own ... ok
test test::events_can_be_filtered_to_the_contract_that_emitted_them ... ok
test test::increment_consumes_a_predictable_amount_of_cpu ... ok
test test::increment_requires_authorization_when_auths_are_not_mocked - should panic ... ok
```

<div class="tk-grid tk-grid--2">
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Pattern</span>
    <h3 class="tk-card__title">One assertion per invocation</h3>
    <p class="tk-card__body">Assert right after the call you care about. The matcher cannot see events from earlier invocations, so an end-of-test count is always wrong.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Pattern</span>
    <h3 class="tk-card__title">Construct per test</h3>
    <p class="tk-card__body">Each test builds its own context. Sharing one environment across tests would leak budget counters and auth records between them.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Pattern</span>
    <h3 class="tk-card__title">Warm up before measuring</h3>
    <p class="tk-card__body">The first cost estimate on a fresh <code>Env</code> can read zero. A throwaway call makes the snapshot meaningful.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Pattern</span>
    <h3 class="tk-card__title">Test the failure path</h3>
    <p class="tk-card__body">A <code>#[should_panic]</code> test with <code>without_mock_auths()</code> is what turns an auth check from a line of code into a guarantee.</p>
  </div>
</div>

<div class="tk-divider"></div>

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Testing patterns</h3>
    <p class="tk-card__body"><a href="./testing-patterns.html">Multi-contract suites</a> and deterministic ledger time.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">testkit-assert</h3>
    <p class="tk-card__body"><a href="../crates/assert.html">Event and auth matchers</a> and their filter semantics.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">testkit-core</h3>
    <p class="tk-card__body"><a href="../crates/core.html">BudgetSnapshot</a> and the <code>BudgetRead</code> trait.</p>
  </div>
</div>
