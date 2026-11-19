# soroban-testkit-core

Budget tracking, error decoding, storage inspection and rent optimization — the primitives the rest of Testkit is built on.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">budget · error · storage · optimizer</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Dependencies</span><span class="tk-spec__value">soroban-sdk · soroban-env-host · serde · serde_json</span></div>
</div>

## API at a glance

| Type | Purpose |
|------|---------|
| `BudgetSnapshot` | CPU instructions and memory bytes — `last_invocation()` for the call that just ran |
| `BudgetRead` | Trait implemented by any source `capture()` can read: invocation resources, the SDK budget, the host `Budget` |
| `BudgetGuard` | Ceilings and a baseline tolerance for one named operation |
| `BudgetViolation`, `ViolationKind` | The limit a cost broke, rendered as one parseable line |
| `BudgetMetric` | Which resource a violation was measured against — `Cpu` or `Memory`, and the `cpu_insns` / `mem_bytes` word `as_str()` prints for it |
| `BudgetBaseline` | Recorded costs per case, loaded from and saved to a JSON file |
| `budget_guard!` | Macro form of the guard around a single invocation |
| `DecodedError` | A Soroban error code split into category, meaning and context |
| `ErrorRegistry` | Your `#[contracterror]` codes mapped to the words they mean |
| `ClientOutcome`, `unwrap_decoded` | Unwrap a v28 `try_*` client call, panicking with the decoded error |
| `StorageEntry`, `StorageTier` | A live storage key, its rendered value, tier and expiry ledger |
| `StorageSnapshot` | Every live entry of one contract, captured at a point in time |
| `StorageDiff`, `StorageChange` | Keys added, removed and rewritten between two snapshots |
| `StorageOptimizationRecommender` | Reads and writes per storage key across a sequence, with tier migrations to match |
| `KeyAccess`, `KeyClass` | How often one key was touched, and whether that pattern makes it `Hot`, `Cold` or `Ephemeral` |
| `AccessThresholds` | Where the boundary between hot, cold and ephemeral sits |
| `RentModel` | The per-byte, per-ledger rent parameters the estimates are computed with |
| `TierRecommendation` | One suggested migration, its rationale, and its projected saving |
| `OptimizationReport` | The access patterns, the recommendations and their totals, rendered as Markdown |

## Budget snapshots

The SDK meters one top-level invocation at a time, and the reading a test wants
is usually the call that just finished:

```rust
use soroban_testkit_core::budget::BudgetSnapshot;

client.increment(&caller, &1);
let cost = BudgetSnapshot::last_invocation(&env);

println!("CPU instructions: {}", cost.cpu_insns);
println!("Memory bytes: {}", cost.mem_bytes);
```

`capture()` takes anything implementing `BudgetRead` — those invocation
resources, the cumulative `env.cost_estimate().budget()`, and a
`soroban_env_host::budget::Budget` you charge by hand. `diff()` saturates at
zero, so a later snapshot that reads smaller never underflows an assertion.

<div class="tk-callout">
  <span class="tk-callout__title">SDK v28 note</span>
  <p><code>env.budget()</code> is deprecated; go through <code>env.cost_estimate()</code>. And read the numbers as a comparison between builds rather than as a fee quote: a contract registered as a native test contract is never metered through the VM, so wasm instantiation, execution and rent reads are absent from the reading.</p>
</div>

## Guards: turning a measurement into a limit

A number on its own passes whatever it is. `BudgetGuard` states the two things a
test can actually act on — an absolute ceiling, and how far the cost may have
grown relative to the last time it was recorded.

```rust
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};

let cost = BudgetSnapshot { cpu_insns: 1_500_000, mem_bytes: 200_000 };

BudgetGuard::new("transfer")          // the name every failure line carries
    .cpu_ceiling(2_000_000)
    .mem_ceiling(500_000)
    .baseline(Some(previous_cost))    // `None` for a case not recorded yet
    .tolerance_percent(10)            // 0 means "not one instruction more"
    .assert_within(&cost);
```

`run()` makes the call, reads the metering that call left behind, and returns
whatever the invocation returned:

```rust
let total = BudgetGuard::new("increment")
    .cpu_ceiling(20_000_000)
    .run(&env, || client.increment(&caller, &1));
```

The `budget_guard!` macro is the same call written around the invocation, with
the limits in a block:

```rust
use soroban_testkit_core::budget_guard;

budget_guard!(&env, "increment", { cpu_max: 20_000_000, mem_max: 20_000_000 }, || {
    client.increment(&caller, &1)
});
```

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Every breach, not just the first</span>
  <p><code>violations()</code> returns all limits one cost breaks — ceilings first, then growth — and <code>assert_within()</code> panics with one line per breach. A change that costs both CPU and memory is found in one run, and the lines are <code>key=value</code> pairs, so CI can grep them.</p>
</div>

```text
BUDGET kind=growth case=transfer metric=cpu_insns actual=1500000 limit=1100000 baseline=1000000 tolerance_percent=10
```

## Baselines: recording cost so a regression is visible

`BudgetBaseline` is a map of case name to recorded cost that reads and writes a
JSON file, so the numbers a suite enforces live in the repository rather than in
a test's memory.

```rust
use soroban_testkit_core::budget::BudgetBaseline;

let baseline = BudgetBaseline::load(std::path::Path::new("tests/budget.json"))?;

for (case, cost) in baseline.cases() {
    baseline.guard(case).tolerance_percent(5).run(&env, || invoke(case));
}
```

`load` and `save` are the file-shaped ends; `parse` and `to_json_string` do the same work on a `&str` and a `String`, which is what a CI job that reads the baseline from an artifact needs. `get(case)` reads one recorded cost, `record(case, cost)` adds or replaces one and returns `&mut Self` so calls chain, and `len` / `is_empty` describe the map.

The file format is one version field and one map of costs:

```json
{
  "version": 1,
  "cases": {
    "increment": { "cpu_insns": 32669, "mem_bytes": 5252 }
  }
}
```

Cases are keyed by name and iterated in name order, so a rewritten file stays a
readable diff. A file whose `version` is newer than the loader is refused with
`BaselineError::UnsupportedVersion` rather than silently read with missing cases.

<div class="tk-callout">
  <span class="tk-callout__title">Recording a baseline</span>
  <p><code>record()</code> and <code>save()</code> write the file; what refreshes it is a policy choice. Committing the file and failing on drift keeps the numbers honest, so most projects regenerate it in one deliberate commit rather than on every test run — <a href="../guides/budget-testing.html">Budget-Aware Testing</a> walks through both. The pattern is applied in this repository: <code>examples/counter/budget.json</code> holds the counter's two hot paths, two <code>#[ignore]</code>d tests record and check it, and the <code>Budget baseline</code> workflow runs the check on one pinned runner and reports the numbers on the pull request.</p>
</div>

## Error decoding

Soroban reports failures as a packed integer. `DecodedError::from_error` splits it into the category the host raised it in and the code's known meaning:

```rust
use soroban_testkit_core::error::DecodedError;

// A v28 client's `try_*` method reports the contract's own error as `Err(Ok(error))`.
let error = client.try_withdraw(&amount).unwrap_err().unwrap();

println!("{}", DecodedError::from_error(&error));
// Soroban Error [storage/3]: MissingValue — a required value was not provided (errors accessing host storage)
```

The ten categories the protocol defines (`contract`, `wasm_vm`, `context`, `storage`, `object`, `crypto`, `events`, `budget`, `value`, `auth`) and the ten standard codes (`ArithDomain`, `IndexBounds`, `InvalidInput`, `MissingValue`, `ExistingValue`, `ExceededLimit`, `InvalidAction`, `InternalError`, `UnexpectedType`, `UnexpectedSize`) are all described. An unrecognised number falls back to a sentence naming it rather than a panic.

### Your own error codes

A `#[contracterror]` enum hands the host an integer with no words attached, so the vocabulary is yours to supply. `ErrorRegistry` is that mapping:

```rust
use soroban_testkit_core::error::{ErrorRegistry, DecodedError};

let registry = ErrorRegistry::new()
    .register(101, "Insufficient balance")
    .register(102, "Contract is paused");

let decoded = DecodedError::from_error_with(&error, &registry);
assert_eq!(decoded.to_string(), "Soroban Error [101]: Insufficient balance");
```

Without a registry a contract code still decodes — it just says the code is unregistered, which beats reading `Error(3)` and going hunting.

### In test output

`unwrap_decoded` unwraps a v28 `try_*` client call and panics with the decoded sentence, so a failing call reads as a diagnosis in the test log rather than as nested `Result` debug output:

```rust
use soroban_testkit_core::error::unwrap_decoded;

let total: u32 = unwrap_decoded(client.try_withdraw(&amount));
// panicked at 'Soroban Error [budget/5]: ExceededLimit — a gas or size limit was hit (errors relating to budget limits)'
```

The nesting you see at that call site — `Err(Ok(error))` — is the SDK's own shape for a client method whose contract function returns `Result`; `ClientOutcome<T>` is the alias Testkit names it with. `unwrap_decoded_with` is the same call against an `ErrorRegistry`, so your own codes come out as words.

`DecodedError` implements `Display`, so it also reads well inside `assert!` messages:

```rust
use soroban_testkit_core::error::DecodedError;

let err = DecodedError {
    code: 12,
    category: "contract",
    message: "Insufficient balance".to_string(),
    context: Some("transfer".to_string()),
};
assert!(format!("{}", err).contains("Insufficient balance"));
```

The rest of the module is small. `DecodedError::from_contract_code(code)` decodes a bare `#[contracterror]` integer as read from `Error::get_code()`, with no error value to unpack, and `label()` renders a code the way the host does — `category/code` for a host error, the bare number for a contract one, which is why the output above reads `[12]` rather than `[contract/12]`. On the registry side, `lookup(code)` reads back a name you registered, `decoded_contract(code)` builds a `DecodedError` from it and says so plainly when the code is not registered, and `len` / `is_empty` describe the map. `CATEGORY_CONTRACT` is the exported constant for the `"contract"` category those compare against.

## Storage snapshots and diffs

Capture everything a contract holds, then compare two captures around an
operation. Keys and values are rendered to stable strings (`u32` becomes `"42"`,
a map becomes `"{a: 1, b: 2}"`), so a diff reports what changed rather than two
opaque blobs. Instance storage is unfolded entry by entry, and each entry
carries the ledger sequence it expires at.

```rust
use soroban_testkit_core::storage::{inspect_storage, StorageSnapshot, StorageTier};

let entries = inspect_storage(&env, &contract);
for entry in &entries {
    println!("{} = {} ({})", entry.qualified(), entry.value, entry.key);
}

let before = StorageSnapshot::capture(&env, &contract);
client.bump();
let after = StorageSnapshot::capture(&env, &contract);

let diff = before.diff(&after);
assert!(diff.added().is_empty() && diff.removed().is_empty());
assert_eq!(diff.modified()[0].after, "43");

// Or assert directly; failures list what actually happened.
before.assert_entry_removed(&after, "pending");
after.assert_unchanged(&StorageSnapshot::capture(&env, &contract));
assert_eq!(after.in_tier(StorageTier::Temporary).len(), 1);
```

### Storage tier assertions

Use `assert_storage_tier!` to assert that a key or pattern belongs to the expected [`StorageTier`]:

```rust
use soroban_testkit_core::assert_storage_tier;
use soroban_testkit_core::storage::StorageTier;

// Assert an exact key
assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);

// Assert all keys matching a wildcard pattern
assert_storage_tier!(&env, &contract, "balances:*", StorageTier::Persistent);
```

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Scope</span>
  <p>A snapshot covers one contract only: other contracts' data, the WASM <code>ContractCode</code> entry and ledger entries the contract does not own are never included. Account addresses own no contract data, so they capture as empty instead of failing.</p>
</div>

## Storage tier optimization

Which tier a key lives in decides what it costs. Temporary entries are billed for
a shorter lifetime than persistent ones, and instance keys ride the single
contract-instance entry, so a call pays no separate ledger entry read for them and
the key carries no per-entry overhead. `StorageOptimizationRecommender` records
how often each key was read and written over a test sequence, classifies the key
from that pattern, and reports the migrations that would lower its rent:

```rust
use soroban_testkit_core::optimizer::StorageOptimizationRecommender;
use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};

let mut recommender = StorageOptimizationRecommender::new();

let before = StorageSnapshot::capture(&env, &contract);
client.transfer(&from, &to, &100);
let after = StorageSnapshot::capture(&env, &contract);

// Writes come from the diff; reads are named by the test that knew them.
recommender.record_writes_from(&before.diff(&after));
recommender.record_read("balances", StorageTier::Persistent, 96);
recommender.record_read("metadata", StorageTier::Persistent, 12);

let report = recommender.report();

for recommendation in report.recommendations() {
    println!(
        "{}: {} -> {} ({} stroops saved)",
        recommendation.key,
        recommendation.current.as_str(),
        recommendation.recommended.as_str(),
        recommendation.saving_stroops(),
    );
}

println!("{}", report.to_markdown());
```

Reads are recorded explicitly because the SDK reports the state a call leaves
behind, not the keys it read. `record_reads_from(&snapshot)` is the coarse form —
every key the snapshot holds counts as read by that step — while `record_read(key,
tier, bytes)` names one key when the test knows which ones the call depended on.
`record_writes_from(&diff)` records a write for every key the diff reports as added
or rewritten.

| Method | Records |
|--------|---------|
| `record_read(key, tier, bytes)` | One read of a key |
| `record_write(key, tier, bytes)` | One write of a key |
| `record_reads_from(&snapshot)` | A read of every key the snapshot holds |
| `record_writes_from(&diff)` | A write for every added or rewritten key |
| `report()` / `to_markdown()` | The patterns, the migrations and their totals |

The classification is a property of the sequence, not of one call:

| Class | When | Recommendation |
|-------|------|----------------|
| `Hot` | `hot_reads` reads or `hot_accesses` accesses | To `instance` when the key is read more than it is written and fits in `singleton_max_bytes` |
| `Ephemeral` | Written once and read at most `ephemeral_max_reads` times | To `temporary`, whose lifetime is shorter |
| `Cold` | Anything else | None: its tier hardly changes its rent |

`AccessThresholds` carries those four numbers with defaults of 3 reads, 4
accesses, 1 read and 128 bytes, so a short sequence can be judged differently:

```rust
use soroban_testkit_core::optimizer::{AccessThresholds, StorageOptimizationRecommender};

let recommender = StorageOptimizationRecommender::new()
    .with_thresholds(AccessThresholds::new().hot_reads(2).hot_accesses(2));
```

The report is Markdown, with one table row per key, one section per migration, and
the totals at the end:

```text
# Storage optimization report

Analyzed **2** storage key(s) over **6** recorded operation(s), with tiers compared on the same rent model.

## Access patterns

| Key | Tier | Reads | Writes | Size (bytes) | Class |
|-----|------|-------|--------|--------------|-------|
| `config` | persistent | 4 | 1 | 12 | hot |
| `pending` | persistent | 1 | 1 | 20 | ephemeral |

## Recommended tier migrations

| Key | From | To | Projected saving (stroops) | Saving |
|-----|------|----|----------------------------|--------|
| `pending` | persistent | temporary | 1,692,058 | 50.0% |
| `config` | persistent | instance | 1,428,268 | 33.3% |

**Projected rent: 6,410,436 → 3,290,110 stroops (3,120,326 saved, 48.7%).**
```

<div class="tk-callout tk-callout--warn">
  <span class="tk-callout__title">The figures are a comparison, not a fee quote</span>
  <p>Rent is <code>bytes × ledgers kept alive × the network's per-byte rate</code>, and every one of those is a network parameter. <code>RentModel</code> carries them as public fields — 16,000 micro-stroops per byte per ledger, TTLs of 3,110,400 ledgers for persistent and instance entries and 1,555,200 for temporary ones, 48 bytes of per-entry metadata — chosen so two tiers compare consistently. Read the numbers as the saving to confirm against the current settings, and override the model with the values you mean before quoting them as fees.</p>
</div>

<div class="tk-callout">
  <span class="tk-callout__title">What the recommendation cannot see</span>
  <p>A pattern says what the sequence did, not what the contract promises: an entry written once and read once may still be needed a year later, and moving it to temporary storage would delete it. Treat <code>Ephemeral</code> as a question about the data being consumed, and <code>Hot</code>-to-instance as a question about instance size — a large or growing instance is rejected by the host, which is why <code>singleton_max_bytes</code> is small.</p>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Next: <a href="./assert.html">soroban-testkit-assert</a> — turn these measurements into readable assertions.</p>
