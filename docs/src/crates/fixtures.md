# soroban-testkit-fixtures

A reusable test context inspired by Foundry's `setUp()`: one place to build the environment, admin and user addresses every test needs.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-fixtures</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">lib · builder · scenario</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Default</span><span class="tk-spec__value">mock_all_auths on</span></div>
</div>

## What `TestContext` gives you

| Field | Value |
|-------|-------|
| `ctx.env` | A fresh `Env`, auth mocked unless you opt out |
| `ctx.admin` | A generated `Address` to act as deployer / owner |
| `ctx.users` | A `Vec<Address>` you extend with `add_user()` |
| `ctx.mock_auths` | The auth policy the context was built with, re-applied by `reset()` |

## Basic usage

```rust
use soroban_testkit_fixtures::TestContext;

let ctx = TestContext::new();
// ctx.env — pre-configured Env with mock_all_auths
// ctx.admin — generated admin address
// ctx.users — empty vec, add users as needed
// ctx.user(i) — safe accessor with diagnostic panic message
```

## Accessing users

Retrieve generated user addresses with `ctx.user(index)`:

```rust
let ctx = TestContextBuilder::new().with_users(3).build();
let alice = ctx.user(0);
let bob = ctx.user(1);
```

When called with an index out of bounds, `ctx.user(i)` panics with a descriptive diagnostic pointing directly at `TestContextBuilder::with_users`:

```text
user index out of bounds: index is 2 but context holds 2 users (did you call TestContextBuilder::with_users(3)?)
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

## Scripting multi-block scenarios

Time-dependent contract logic — vesting, streams, bridge relays — fails under
ledger movement a single call cannot reproduce: a missed block, a stalled
network, a rollback that makes the clock run backwards. `ScenarioBuilder`
scripts that movement as a chain over one `TestContext`:

```rust
use soroban_testkit_fixtures::ScenarioBuilder;

let mut scenario = ScenarioBuilder::new(&ctx);
scenario
    .step("Open stream", |ctx| stream_client.open(&1_000))
    .advance_blocks(100)
    .step("Claim partial", |ctx| stream_client.claim())
    .halt(3_600)
    .advance_blocks(500)
    .step("Final claim", |ctx| stream_client.claim());

scenario.assert_all_passed();
```

| Method | Effect |
|--------|--------|
| `step(label, f)` | Runs a contract invocation and records it |
| `check(label, f)` | Runs a `bool` state-consistency check |
| `probe(label, f)` | Reads a contract clock/state and rejects a backwards move |
| `advance_blocks(n)` | Closes `n` ledgers, leaving the clock alone |
| `advance_time(s)` | Moves the clock `s` seconds, leaving the sequence alone |
| `advance(blocks, seconds)` | Moves both |
| `halt(s)` | A network halt: time passes, no block closes |
| `rollback_blocks(n)` | A reorg that rewinds the sequence |
| `set_clock(seq, t)` | Jump to an explicit ledger position |

`step` and `check` run through `catch_unwind`, so a contract panic does not stop
the script: later steps still run, and `assert_all_passed` fails with the whole
timeline, marking the step that panicked. `format_timeline()` returns the same
text when a test wants to assert on it:

```text
SCENARIO FAILED (1 failure(s))
#00 seq=0 t=1700000000 start: seq=0 t=1700000000 (start)
#01 seq=0 t=1700000000 Open stream: seq 0->0, t 1700000000->1700000000 (passed)
#02 seq=100 t=1700000000 advance 100 block(s): seq 0->100 (passed)
#03 seq=100 t=1700000100 Claim partial: ... (FAILED: attempt to subtract with overflow)
  -> scenario step #3 "Claim partial": attempt to subtract with overflow
```

`probe` is the automated clock-inversion check. A contract reports the time it
believes it is at, and a reading below the previous one is recorded as a
failure naming both values and the subtraction that would underflow — which is
what catches a streaming contract that computes `now - last_claim` after a
reorg.

```rust
scenario
    .step("Open", |_| stream.open(&1_700_000_000, &10))
    .advance_time(100)
    .probe("before reorg", |_| stream.clock())
    .set_clock(0, 1_699_999_900)
    .probe("after reorg", |_| stream.clock());
// one failure: clock inversion in "after reorg": 1699999900 after …
```

<div class="tk-callout">
  <span class="tk-callout__title">Bursts and halts are the same clock, different chain</span>
  <p>Block production is not uniform. A scenario can close 1 then 100 then 500 blocks, or stop closing them while seconds still pass. A contract that measures progress in <code>timestamp()</code> reaches the same state either way; one that confuses heights with time does not — and the timeline shows which steps saw which heights.</p>
</div>

`blocks_elapsed()` and `seconds_elapsed()` saturate at zero, so a rollback below
the starting height reports zero rather than wrapping into a huge number.

## Resetting the chain underneath a test

Some scenarios are the same actors on a clean chain twice: a phase-one
`initialize`, then a phase-two that must not see phase-one's storage. `reset`
gives you that without rebuilding the fixture:

| Method | Effect |
|--------|--------|
| `ctx.reset()` | Fresh env — ledger, events, auths and contract storage all cleared — with the same admin and users |
| `ctx.reset_full()` | Fresh env *and* new identities: a new admin and a new `users` list of the same length |

```rust
let mut ctx = TestContextBuilder::new().with_users(2).build();

let env = ctx.env.clone();
let (vault, client) = register_vault(&env);
client.deposit(&ctx.users[0], &1_000);

ctx.reset();
// ctx.admin and ctx.users are the same actors; the chain remembers nothing.
// `vault` named a contract in the env that is gone, so register again:
let env = ctx.env.clone();
let (vault, client) = register_vault(&env);
assert_eq!(client.balance(&vault, &ctx.users[0]), 0);
```

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Identities are carried, registrations are not</span>
    <h3 class="tk-card__title">Re-register after resetting</h3>
    <p class="tk-card__body">An <code>Address</code> is a handle into the env that made it, so <code>reset</code> rebuilds each one from its <code>ScAddress</code> form and the strkey survives intact. A <em>registered contract</em> cannot travel that way — its code and storage live in the old env — so the id you held before the reset points at nothing. Reach for a second <code>TestContext</code> instead whenever the two phases want different fixtures, user counts or auth policies.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Authorisation policy</span>
    <h3 class="tk-card__title">Reset keeps the policy, not by magic</h3>
    <p class="tk-card__body"><code>TestContext</code> remembers whether it mocks authorizations, so an unmocked context stays unmocked through a reset. An <code>Env</code> does not report its own policy, which means <code>with_env</code> has to assume the mocked default — set <code>ctx.mock_auths = false</code> when it is not what you configured.</p>
  </div>
</div>

`reset_full` exists because SDK v28 mints test addresses from a counter that
restarts at 1 in every `Env`. Generating straight away would hand back exactly
the addresses you replaced, so it first walks the new env's counter past the
range the old identities occupied.

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
    <h3 class="tk-card__title">Behind <code>testutils</code></h3>
    <p class="tk-card__body"><code>Address::generate</code> is a test-only SDK API. SDK v28 requires <code>use soroban_sdk::testutils::Address as _;</code> plus the <code>testutils</code> feature.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Roadmap</span>
    <h3 class="tk-card__title">Seeded state</h3>
    <p class="tk-card__body">Registering contracts and seeding their state from the builder would turn the setup above into one line.</p>
  </div>
</div>
