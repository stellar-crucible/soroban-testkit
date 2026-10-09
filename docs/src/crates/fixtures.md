# soroban-testkit-fixtures

A reusable test context inspired by Foundry's `setUp()`: one place to build the environment, admin and user addresses every test needs.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-fixtures</span></div>
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
use soroban_testkit_fixtures::TestContext;

let ctx = TestContext::new();
// ctx.env — pre-configured Env with mock_all_auths
// ctx.admin — generated admin address
// ctx.users — empty vec, add users as needed
```

## Builder pattern

Describe the context you want instead of assembling it:

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;

let ctx = TestContextBuilder::new()
    .with_users(5)            // Pre-generate 5 user addresses
    .without_mock_auths()     // Disable automatic auth mocking
    .build();
```

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">When to opt out of mocked auth</span>
  <p>Keep <code>mock_all_auths</code> for behaviour tests. Call <code>without_mock_auths()</code> as soon as a test asserts <em>who</em> had to sign — that is <a href="./assert.html">AuthMatcher</a> territory.</p>
</div>

## Moving the ledger clock

Time-dependent contracts — vesting, auctions, lockups — need the ledger to
advance between two calls. `TestContext` exposes the four operations that cover
almost all of that without building a `LedgerInfo` by hand:

| Method | Effect |
|--------|--------|
| `ctx.timestamp()` | The unix timestamp the ledger reports |
| `ctx.sequence()` | The ledger sequence number the ledger reports |
| `ctx.set_timestamp(t)` | Jump to absolute time `t` |
| `ctx.advance_time(s)` | Add `s` seconds, return the new timestamp |
| `ctx.advance_ledger(n)` | Add `n` to the sequence, return the new height |

```rust
use soroban_testkit_fixtures::TestContext;

let mut ctx = TestContext::new();
ctx.set_timestamp(1_700_000_000);
// ... assert the position is locked

let unlocked_at = ctx.advance_time(86_400 * 30); // 30 days later
assert_eq!(unlocked_at, 1_702_592_000);
// ... assert the position is claimable
```

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Time and height are separate</span>
    <h3 class="tk-card__title">Advance only what you assert</h3>
    <p class="tk-card__body"><code>advance_time</code> leaves the sequence alone and <code>advance_ledger</code> leaves the clock alone. A real ledger close moves both, so a test that wants the full picture calls each — and a test that only cares about TTL expiry does not silently move the clock.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">A fresh env opens at zero</span>
    <h3 class="tk-card__title">Set before you advance</h3>
    <p class="tk-card__body">The SDK test env starts at timestamp <code>0</code> and sequence <code>0</code>, so <code>advance_time(3_600)</code> lands on <code>3_600</code> rather than on wall-clock time. Pin an absolute moment with <code>set_timestamp</code> first when the test reads better as a date.</p>
  </div>
</div>

Both helpers saturate instead of wrapping: advancing past `u64::MAX` or
`u32::MAX` returns the maximum rather than panicking or rolling the clock back
to a date in 1970.

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
    <p class="tk-card__body">Registering contracts and seeding their state from the builder would turn the setup above into one line. Tracked as <a href="https://github.com/stellar-crucible/soroban-testkit/issues/6">issue #6</a>.</p>
  </div>
</div>
