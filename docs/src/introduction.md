<div class="tk-hero">
  <p class="tk-hero__eyebrow">Stellar &middot; Soroban &middot; Test tooling</p>
  <h1 class="tk-hero__title" id="soroban-testkit">Soroban Testkit</h1>
  <p class="tk-hero__lede"><strong>Soroban Testkit</strong> is an open-source testing and debugging framework for <a href="https://developers.stellar.org/docs/smart-contracts">Soroban</a> smart contracts. It layers ergonomic fixtures, assertions, budget introspection and property-testing strategies on top of the official SDK, so contract tests are shorter, faster to write, and far closer to how they behave on-chain.</p>
  <div class="tk-hero__actions">
    <a class="tk-btn tk-btn--primary" href="./getting-started/quick-start.html">Quick start</a>
    <a class="tk-btn tk-btn--ghost" href="https://github.com/stellar-crucible/soroban-testkit">View on GitHub</a>
    <a class="tk-btn tk-btn--ghost" href="./contributing.html">Contribute &amp; earn</a>
  </div>
  <div class="tk-badges">
    <span class="tk-badge tk-badge--brand">Soroban SDK v28</span>
    <span class="tk-badge">Rust 1.91+</span>
    <span class="tk-badge">4 workspace crates</span>
    <span class="tk-badge tk-badge--accent">Stellar Wave funded</span>
  </div>
</div>

## Why Testkit

The Soroban SDK ships solid primitives — `Env::default()`, `testutils`, event inspection — but day-to-day contract testing still has rough edges that every team re-solves on its own.

<div class="tk-problems">
  <div class="tk-problem">
    <span class="tk-problem__label">Verbose setup</span>
    <span class="tk-problem__body">Every test manually creates an environment, registers contracts, generates addresses and seeds state.</span>
    <span class="tk-problem__fix">Solved by <b>soroban-testkit-fixtures</b> — a Foundry-style `setUp` context builder.</span>
  </div>
  <div class="tk-problem">
    <span class="tk-problem__label">Raw assertions</span>
    <span class="tk-problem__body">Matching events and authorizations means destructuring tuples by hand with no domain helpers.</span>
    <span class="tk-problem__fix">Solved by <b>soroban-testkit-assert</b> — chainable event and auth matchers.</span>
  </div>
  <div class="tk-problem">
    <span class="tk-problem__label">Hidden resource costs</span>
    <span class="tk-problem__body">Tests pass locally yet fail on-chain once the CPU or memory budget is exhausted.</span>
    <span class="tk-problem__fix">Solved by <b>soroban-testkit-core</b> — budget snapshots and diffs around any call.</span>
  </div>
  <div class="tk-problem">
    <span class="tk-problem__label">Heavyweight mocking</span>
    <span class="tk-problem__body">Stubbing a single dependency forces you to write and register an entire mock contract.</span>
    <span class="tk-problem__fix">Solved by <b>soroban-testkit-core</b> — error decoding and storage inspection helpers.</span>
  </div>
  <div class="tk-problem">
    <span class="tk-problem__label">No property-testing support</span>
    <span class="tk-problem__body">Soroban-aware generators have to be rebuilt from scratch in every project.</span>
    <span class="tk-problem__fix">Solved by <b>soroban-testkit-generators</b> — `proptest` and `arbitrary` strategies for SDK types.</span>
  </div>
</div>

## The Crates

Each crate is independent — adopt one, or combine all four for a complete testing stack.

<div class="tk-grid tk-grid--4">
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-core</span>
    <h3 class="tk-card__title">Inspect &amp; measure</h3>
    <p class="tk-card__body">Budget snapshots with call-level diffs, on-chain error decoding, and storage inspection across instance, persistent and temporary tiers.</p>
    <div class="tk-card__meta"><span class="tk-badge">BudgetSnapshot</span><span class="tk-badge">DecodedError</span></div>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-assert</span>
    <h3 class="tk-card__title">Assert fluently</h3>
    <p class="tk-card__body">Readable matchers for contract events and authorizations, filterable by contract id and topic.</p>
    <div class="tk-card__meta"><span class="tk-badge">EventMatcher</span><span class="tk-badge">AuthMatcher</span></div>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-fixtures</span>
    <h3 class="tk-card__title">Set up once</h3>
    <p class="tk-card__body">A reusable test context plus a builder for environments, admin accounts and generated users.</p>
    <div class="tk-card__meta"><span class="tk-badge">TestContext</span><span class="tk-badge">TestContextBuilder</span></div>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-generators</span>
    <h3 class="tk-card__title">Generate wildly</h3>
    <p class="tk-card__body">Property-testing strategies for token amounts, ledger sequences and timestamps that respect Soroban limits.</p>
    <div class="tk-card__meta"><span class="tk-badge">proptest</span><span class="tk-badge">arbitrary</span></div>
  </div>
</div>

## Architecture

```text
soroban-testkit/                    Cargo workspace, resolver 2
├── crates/soroban-testkit-core             budget snapshots · error decoding · storage
├── crates/soroban-testkit-assert           event matchers · authorization matchers
├── crates/soroban-testkit-fixtures         test context · context builder
└── crates/soroban-testkit-generators       proptest strategies · arbitrary impls
```

## A First Look

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">Six lines to a real test</span>
  <p>Build a context, register the contract, invoke a call, then assert on events and budget in the same breath.</p>
</div>

```rust
use soroban_testkit_assert::events::EventMatcher;
use soroban_testkit_fixtures::TestContext;

#[test]
fn transfer_emits_event() {
    let ctx = TestContext::new();
    // arrange: register your contract against ctx.env
    // act: invoke the contract client

    EventMatcher::new(&ctx.env).assert_emitted();
}
```

Full walkthroughs live in [Quick Start](./getting-started/quick-start.md), [Testing Patterns](./guides/testing-patterns.md) and [Budget-Aware Testing](./guides/budget-testing.md).

## Stellar Wave

<div class="tk-cta">
  <p>This project is funded through the <a href="https://www.drips.network/wave/stellar">Stellar Wave</a> program on <a href="https://www.drips.network">Drips Network</a>. Every issue carries a complexity label — <span class="tk-badge tk-badge--brand">Trivial 100 pts</span> <span class="tk-badge tk-badge--brand">Medium 150 pts</span> <span class="tk-badge tk-badge--brand">High 200 pts</span> — and rewards land once your PR is merged.</p>
  <a class="tk-btn tk-btn--primary" href="https://github.com/stellar-crucible/soroban-testkit/issues">Pick up an issue</a>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Built and maintained by <a href="https://github.com/stellar-crucible">Stellar Crucible</a> · Apache-2.0 licensed · contributions welcome.</p>
