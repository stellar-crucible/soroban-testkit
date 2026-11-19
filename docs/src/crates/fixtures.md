# soroban-testkit-fixtures

A reusable test context inspired by Foundry's `setUp()`: one place to build the environment, admin and user addresses every test needs.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-fixtures</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">lib · builder · migration · event_indexer · gas_exhaustion · congestion</span></div>
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

## Testing schema migrations

Contracts bump their storage schema — v1 to v2 to v3 — and migrate the data
they already hold. `MigrationRunner` drives those migrations from a test and
reads the schema version out of the contract's instance storage before and
after every step:

```rust
use soroban_testkit_fixtures::{MigrationRunner, TestContext};

let ctx = TestContext::new();
let id = ctx.env.register(Vault, ());
let mut runner = MigrationRunner::with_client(&ctx.env, &id, VaultClient::new(&ctx.env, &id));
assert_eq!(runner.current_version(), 1);

let step = runner.execute_migration(2, |client| client.migrate_v1_to_v2());
step.assert_renamed("balance", "ledger");
step.assert_schema_after(&["ledger", "version"]);
assert_eq!(runner.current_version(), 2);

runner.execute_migration(3, |client| client.migrate_v2_to_v3());
runner.assert_schema(&["fee_bps", "ledger", "version"]);
```

`MigrationRunner::new(&env, &id)` is the same runner with the contract's
`Address` passed to each closure instead of a client.

The version lives under the instance-storage symbol `version` by default;
`with_version_key(DataKey::Version)` points the runner at whatever key the
contract uses, `stored_version()` reads it raw, and a contract that stores no
version yet counts as v1 unless `assuming_unversioned_is(n)` says otherwise.
The contract's migration writes the new version itself — the runner checks it,
it never writes it.

| A step is refused or fails when… | `MigrationError` |
|----------------------------------|------------------|
| it already ran through this runner | `AlreadyApplied` — names the step it ran as |
| the target is at or below the current version | `NotForward` |
| the target skips a version (`v1` → `v3`) | `SkipsVersion` |
| it finished without recording the target version | `VersionNotRecorded` |
| it panicked and storage is unchanged | `Failed` |
| it panicked and left storage changed | `NotAtomic` — lists the leftover keys |

The first three are checked before the closure runs, so a double migration
never reaches the contract. `execute_migration` panics with the error's message;
`try_execute_migration` returns it, which is what a test that *expects* a
refusal wants.

Each successful step returns a `MigrationRecord` holding the `StorageSnapshot`
on either side of it. `diff()`, `assert_added`, `assert_removed`,
`assert_renamed`, `assert_schema_before` and `assert_schema_after` are the
before-and-after schema assertions, and `runner.history()` lists every step in
order.

<div class="tk-callout">
  <span class="tk-callout__title">Atomic by construction, verified anyway</span>
  <p>A single contract call that panics is rolled back by the host, so a broken one-call migration reports <code>Failed</code> with storage untouched. A migration spread over several calls has no such transaction around it: if the second call fails, the first call's writes stay — and the runner reports that as <code>NotAtomic</code>, naming what was left behind.</p>
</div>

## EventIndexerEmulator

`EventIndexerEmulator` is an in-memory index that belongs to a test scenario:
keep one instance for as long as the test needs its history, and drop it when
the scenario ends. SDK v28's `env.events().all()` exposes the events from the
most recent invocation, not an automatic stream of all ledgers. Ingest the
events after each invocation and provide the ledger sequence explicitly:

```rust
use soroban_sdk::testutils::{Events as _, Ledger as _};
use soroban_testkit_fixtures::EventIndexerEmulator;

let mut indexer = EventIndexerEmulator::new();
let sequence = ctx.sequence();
client.transfer(&from, &to, &100);
indexer
    .ingest_ledger(sequence, ctx.env.events().all().events())
    .unwrap();

ctx.advance_ledger(1);
let sequence = ctx.sequence();
client.transfer(&from, &to, &200);
indexer
    .ingest_ledger(sequence, ctx.env.events().all().events())
    .unwrap();
```

Ingestion copies the original `ContractEvent`, including only metadata the SDK
actually exposes. The ledger sequence is fixture-level metadata supplied by
the caller; it is not present in the event itself. Soroban also does not give
these events a unique event id, so the fixture identity is the supplied ledger
sequence plus a monotonically increasing ingestion ordinal. Re-ingesting the
same XDR event records a second occurrence rather than guessing that it is a
duplicate.

The index has inverted indexes for symbol topics and emitting contract ids,
plus an ordered ledger-sequence index. Results are ordered by ascending ledger
sequence and then by ingestion order within a ledger, even if the test ingests
ledgers out of order. A query combines topic, contract and ledger filters with
AND; an event matching multiple of its own topics appears only once.

### Topic, contract and ledger queries

An exact topic pattern matches a `ScVal::Symbol` topic exactly. `*` matches
zero or more bytes and can be used anywhere in the pattern: `"transfer"` is
exact, `"trans*"` matches a prefix, `"*fer"` matches a suffix, and `"*"` matches
any symbol topic. Non-symbol topic values are not interpreted as strings and
do not match a topic query.

`query_by_contract` accepts a Soroban contract `Address`; account addresses
match no events. `query_by_contract_id` accepts an XDR `ContractId` directly.
`query_range` treats both ledger endpoints as inclusive and returns an error
when the start is greater than the end.

```rust
use soroban_testkit_fixtures::{EventIndexerEmulator, EventQuery};

let transfers = indexer.query_by_topic("transfer");
let matching_contract = indexer.query_by_contract(&contract_id);
let ledgers = indexer.query_range(100, 110)?;

let query = EventQuery::new()
    .topic("trans*")
    .contract_address(&contract_id)
    .ledger_range(100, 110);
let first_page = indexer.query(&query)?;
```

Each query returns an `EventPage`. Its `total` is the count before pagination,
and `offset` identifies the first requested match. The default maximum page
size is 100; configure a positive cap with `set_page_limit`. Explicit page
requests use `query_by_topic_page`, `query_by_contract_page`,
`query_range_page`, or `query_page`; a requested size above the cap is reduced
to the configured maximum. Use `next_offset()` to fetch the next page:

```rust
indexer.set_page_limit(25)?;
let first = indexer.query_by_topic("transfer");
if let Some(offset) = first.next_offset() {
    let next = indexer.query_by_topic_page("transfer", offset, 25)?;
    // `next.events` continues in the same deterministic order.
}
```

Empty queries return an empty page. Zero page sizes and reversed ledger ranges
are reported as `EventIndexerError`, not silently accepted.

## GasExhaustionHarness

`GasExhaustionHarness` executes a real Soroban host invocation after applying
the configured CPU and memory limits through SDK v28's test budget. It does
not synthesize budget errors. Its default configuration is 250,000 CPU
instructions and unlimited test-host memory; `cpu_50k()` and `cpu_100k()`
provide the other common CPU ceilings. Use `with_limits(cpu, memory)` for a
custom pair.

The function name and arguments are passed to `Env::try_invoke_contract`, so
arguments must be Soroban `Val`s. The report distinguishes successful results,
contract errors, other host errors, invocation aborts, conversion failures and
host-reported budget exhaustion:

```rust
use soroban_sdk::{Env, Val, Vec};
use soroban_testkit_fixtures::{
    ExecutionStatus, GasExhaustionHarness,
};

let env = Env::default();
let contract_id = env.register(MyContract, ());
let report = GasExhaustionHarness::cpu_250k().invoke(
    &env,
    &contract_id,
    "process_batch",
    Vec::<Val>::new(&env),
);

match &report.status {
    ExecutionStatus::Succeeded(_) => {}
    ExecutionStatus::BudgetExhausted { resource, .. } => {
        eprintln!("execution exhausted {resource:?}");
    }
    other => panic!("unexpected invocation result: {other:?}"),
}
println!("{}", report.diagnostic());
```

The report includes the configured limits, actual CPU and memory costs, and
diagnostic events returned by `env.logs().all()`. It classifies exhaustion by
comparing the measured resource totals with those limits; if both dimensions
cross the boundary or the counters do not identify one, the report says so
instead of guessing. The harness resets the test budget to SDK defaults after
recording the report, so later assertions and storage inspection can proceed.

Failed invocations use the host's transactional semantics. To test rollback,
capture state before the call, invoke a contract method that writes persistent
state and then performs enough metered host work to exhaust its budget, and
compare the state snapshot after the harness returns:

```rust
use soroban_testkit_core::storage::StorageSnapshot;
use soroban_sdk::{Env, IntoVal, Val};
use soroban_testkit_fixtures::{ExecutionStatus, GasExhaustionHarness};

let before = StorageSnapshot::capture(&env, &contract_id);
let report = GasExhaustionHarness::with_limits(100_000, u64::MAX).invoke(
    &env,
    &contract_id,
    "write_then_process_large_batch",
    soroban_sdk::vec![&env, 10_000u32.into_val(&env)],
);
assert!(matches!(
    &report.status,
    ExecutionStatus::BudgetExhausted { .. }
));
assert_eq!(StorageSnapshot::capture(&env, &contract_id), before);
```

For memory-specific tests, use a custom CPU limit with a deliberately tight
memory limit. The host can report resource exhaustion and the metered totals,
but it does not expose an exact Rust source statement or loop iteration.
Native test contracts also omit VM instruction and Wasm execution costs, so
their measurements are not representative of the deployed Wasm contract;
register or upload Wasm when those costs are part of the test.

## Congestion and queue delay simulator

Under network congestion on Stellar, transaction base fees fluctuate and transactions can be delayed across multiple ledgers in validator queues before execution. Contracts enforcing deadlines (such as slippage parameters or auction bidding windows) must reject delayed transactions gracefully. `CongestionSimulator` emulates variable base fees, surge spikes, and ledger delays:

```rust
use soroban_testkit_fixtures::CongestionSimulator;

let mut sim = CongestionSimulator::new()
    .with_base_fee(100)                     // Base fee in Stroops
    .with_base_fee_range(100, 1_000)        // Fluctuation bounds
    .with_surge_multiplier(5)               // Surge pricing multiplier
    .with_delay_range(1, 10)                // Simulated ledger queue arrival delay
    .with_seconds_per_ledger(5);

// Check fee requirements under fluctuating conditions:
let required_fee = sim.calculate_fee(1);
assert!(sim.is_fee_sufficient(offered_fee, 1));

// Advance ledger height and timestamp to simulate arrival queue latency:
let report = sim.advance_delay(&env);
println!("Delayed by {} ledgers ({}s)", report.ledgers_delayed, report.seconds_delayed);

// Execute invocation closure with simulated congestion delay:
let (report, result) = sim.execute_with_delay(&env, || {
    client.try_swap(&100, &90, &deadline)
});
```

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
