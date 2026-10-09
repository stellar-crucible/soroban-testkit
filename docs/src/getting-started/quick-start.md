# Quick Start

Write a complete Soroban test in about a minute: build a context, register the contract, invoke it, then assert on events and resource cost in the same test.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Time</span><span class="tk-spec__value">~5 minutes</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates used</span><span class="tk-spec__value">fixtures · assert · core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Prerequisite</span><span class="tk-spec__value"><a href="./installation.html">Installation</a> done</span></div>
</div>

## Imports

```rust
use soroban_sdk::{Env, Address};
use testkit_fixtures::builder::TestContextBuilder;
use testkit_assert::events::EventMatcher;
use testkit_core::budget::BudgetSnapshot;
```

## Write a test

<ol class="tk-steps">
  <li>
    <h4>Build the context</h4>
    <p><code>TestContextBuilder</code> hands you a mocked <code>Env</code> plus ready-made addresses.</p>
  </li>
  <li>
    <h4>Register the contract</h4>
    <p>Bind your contract to the context environment and create its client.</p>
  </li>
  <li>
    <h4>Snapshot the budget</h4>
    <p>Capture CPU and memory before the call so you can diff it afterwards.</p>
  </li>
  <li>
    <h4>Invoke and assert</h4>
    <p>Call the function under test, then match events and check the resource delta.</p>
  </li>
</ol>

```rust
#[test]
fn test_token_transfer() {
    // 1. Create a test context with pre-generated users
    let ctx = TestContextBuilder::new()
        .with_users(2)
        .build();

    let sender = &ctx.users[0];
    let receiver = &ctx.users[1];

    // 2. Register your contract
    let contract_id = ctx.env.register(MyContract, ());
    let client = MyContractClient::new(&ctx.env, &contract_id);

    // 3. Capture budget before invocation
    let before = BudgetSnapshot::capture(&ctx.env.cost_estimate().budget());

    // 4. Invoke the function under test
    client.transfer(sender, receiver, &1000);

    // 5. Assert events were emitted
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("Transfer")
        .assert_emitted();

    // 6. Check resource consumption
    let after = BudgetSnapshot::capture(&ctx.env.cost_estimate().budget());
    let diff = before.diff(&after);
    assert!(diff.cpu_insns < 500_000);
}
```

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">Expected output</span>
  <p>The test passes with no extra scaffolding — no manual <code>Env</code> construction, no tuple destructuring over <code>env.events().all()</code>, no separate budget harness.</p>
</div>

## What's happening

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">TestContextBuilder</span>
    <h3 class="tk-card__title">Setup, once</h3>
    <p class="tk-card__body">Eliminates manual <code>Env</code> creation, address generation and auth mocking. Opt out of mocked auth when a test needs explicit authorization.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">EventMatcher</span>
    <h3 class="tk-card__title">Readable assertions</h3>
    <p class="tk-card__body">Replaces raw tuple iteration over emitted events with a chainable filter by contract and topic.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">BudgetSnapshot</span>
    <h3 class="tk-card__title">Costs made visible</h3>
    <p class="tk-card__body">Surfaces CPU-instruction and memory deltas that standard Soroban tests never show — the ones that fail on-chain.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Composable</span>
    <h3 class="tk-card__title">Adopt piecemeal</h3>
    <p class="tk-card__body">Each crate stands alone. Start with fixtures, add assertions when your tests grow, keep budget checks for CI.</p>
  </div>
</div>

## Next steps

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <h3 class="tk-card__title"><a href="../guides/testing-patterns.html">Testing patterns</a></h3>
    <p class="tk-card__body">Layouts for unit, integration and contract-interaction tests.</p>
  </div>
  <div class="tk-card">
    <h3 class="tk-card__title"><a href="../guides/budget-testing.html">Budget-aware testing</a></h3>
    <p class="tk-card__body">Turn resource regressions into ordinary failing assertions.</p>
  </div>
  <div class="tk-card">
    <h3 class="tk-card__title"><a href="../guides/property-testing.html">Property testing</a></h3>
    <p class="tk-card__body">Generate Soroban-aware inputs with proptest strategies.</p>
  </div>
</div>
