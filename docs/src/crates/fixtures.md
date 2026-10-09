# testkit-fixtures

A reusable test context inspired by Foundry's `setUp()`: one place to build the environment, admin and user addresses every test needs.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">testkit-fixtures</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">lib · builder</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Default</span><span class="tk-spec__value">mock_all_auths on</span></div>
</div>

## What `TestContext` gives you

| Field | Value |
|-------|-------|
| `ctx.env` | A fresh `Env`, auth mocked unless you opt out |
| `ctx.admin` | A generated `Address` to act as deployer / owner |
| `ctx.users` | A `Vec<Address>` you extend with `add_user()` |

## Basic usage

```rust
use testkit_fixtures::TestContext;

let ctx = TestContext::new();
// ctx.env — pre-configured Env with mock_all_auths
// ctx.admin — generated admin address
// ctx.users — empty vec, add users as needed
```

## Builder pattern

Describe the context you want instead of assembling it:

```rust
use testkit_fixtures::builder::TestContextBuilder;

let ctx = TestContextBuilder::new()
    .with_users(5)            // Pre-generate 5 user addresses
    .without_mock_auths()     // Disable automatic auth mocking
    .build();
```

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">When to opt out of mocked auth</span>
  <p>Keep <code>mock_all_auths</code> for behaviour tests. Call <code>without_mock_auths()</code> as soon as a test asserts <em>who</em> had to sign — that is <a href="./assert.html">AuthMatcher</a> territory.</p>
</div>

## Extending fixtures

Compose `TestContext` into project-specific fixtures so setup is written once per repo:

```rust
struct TokenTestEnv {
    ctx: TestContext,
    token_id: BytesN<32>,
}

impl TokenTestEnv {
    fn setup() -> Self {
        let ctx = TestContextBuilder::new().with_users(3).build();
        let token_id = ctx.env.register(TokenContract, ());
        Self { ctx, token_id }
    }
}
```

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Address generation</span>
    <h3 class="tk-card__title">Behind `testutils`</h3>
    <p class="tk-card__body"><code>Address::generate</code> is a test-only SDK API. SDK v28 requires <code>use soroban_sdk::testutils::Address as _;</code> plus the <code>testutils</code> feature.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Roadmap</span>
    <h3 class="tk-card__title">Seeded state</h3>
    <p class="tk-card__body">Declarative state seeding is an open wave issue — see the <a href="https://github.com/stellar-crucible/soroban-testkit/issues">issue tracker</a> to pick it up.</p>
  </div>
</div>
