# soroban-testkit-fixtures

A reusable test context inspired by Foundry's `setUp()`: one place to build the environment, admin and user addresses every test needs.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-fixtures</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">lib · builder · archival</span></div>
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
| `ctx.archive_expired(n)` | Advance `n` ledgers, archive everything past its TTL, return the `ArchivalSweepSimulator` holding it |

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

## Archival sweeps

On a real network an entry lives only as long as its rent. When the rent runs
out, a persistent entry is *archived* — still on the ledger, but unreadable
until a restore operation pays to bring it back — and a temporary entry is
deleted for good. The SDK test environment does not reproduce that: it
re-extends an expired persistent entry the moment a test reads it, so a test
that wants to see the failure has to perform the sweep itself.

`ArchivalSweepSimulator` does that against the whole environment, every
registered contract at once:

```rust
use soroban_testkit_fixtures::archival::ArchivalSweepSimulator;

let mut simulator = ArchivalSweepSimulator::new();
let sweep = simulator.sweep(&env, 5_000);   // advance 5,000 ledgers, then sweep

assert_eq!(sweep.archived, 1);               // the persistent entry is gone
assert_eq!(client.balance(&holder), None);   // the contract reads it as missing
assert!(client.try_withdraw(&holder).is_err());

let restored = simulator.restore_all(&env, 5_000); // a restore operation
assert_eq!(restored, 1);
assert_eq!(client.balance(&holder), Some(1_000));  // the original data is back
```

| Method | Effect |
|--------|--------|
| `sweep(&env, ledgers_ahead)` | Advance the ledger, then sweep; returns an `ArchivalSweep` |
| `sweep_expired(&env)` | Sweep at the current sequence without moving the ledger |
| `archived()`, `len()`, `is_archived(key)`, `entry(key)` | What the sweep took, and the `ArchivedEntry` for one key |
| `restore_all(&env, extra_ttl)` | Put every archived entry back, returning how many were restored |
| `restore(&env, key, extra_ttl)` | Put one entry back; `false` when the sweep never took it |
| `assert_archived(&env, key)`, `assert_restored(&env, key)` | Fail unless the ledger agrees the entry is gone, or back |

An `ArchivedEntry` carries the exact `LedgerEntry` the host held, not a
re-encoding of it, so a restore hands the contract back the data it wrote — the
unit tests compare the value, the rendered storage snapshot and the restored
TTL, not merely that a read succeeded. `extra_ttl` is the rent the restore buys;
a restore always grants at least one ledger, so an entry restored with `0` is not
immediately eligible for the next sweep.

```rust
let sweep = simulator.sweep(&env, 5_000);
assert_eq!(sweep.ledger, 5_000);
assert_eq!(sweep.evicted_temporary, 1);   // temporary entries die, unarchived
assert_eq!(sweep.kept_live, 1);           // and everything inside its TTL stays
assert_eq!(simulator.restore_all(&env, 5_000), 0); // nothing comes back for those
```

<div class="tk-callout">
  <span class="tk-callout__title">Reading an archived entry is StorageNotFound</span>
  <p>An archived entry is absent from the ledger: a contract's <code>persistent().get()</code> returns <code>None</code>, <code>has()</code> returns <code>false</code>, and an operation that requires the entry — extending its TTL, restoring it from inside a call — fails with the host's <code>Storage/MissingValue</code>. Because a native test contract is a guest call, the env narrows that code at the boundary to <code>Context/InvalidAction</code>, which is what a <code>try_*</code> client method reports; inspect the storage state or use <code>assert_archived</code> when the test needs to name the absence itself.</p>
</div>

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">The contract stays callable</span>
  <p>A sweep skips the contract's instance entry and its code entry, so the contract can still be invoked while its data entries are archived — otherwise the test could not observe what the sweep did. An expired instance entry is left as it is and the host re-extends it on the next call.</p>
</div>

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">Do not hard-code the rent</span>
  <p>How many ledgers an entry has left is network configuration, not a constant a test should write down. Read it back from <code>StorageSnapshot::capture(&env, &contract).get("balance").live_until</code> and advance past it, or sweep far enough with <code>ctx.archive_expired(10_000)</code> when the exact figure does not matter.</p>
</div>

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
