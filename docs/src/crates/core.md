# soroban-testkit-core

Budget tracking, error decoding and storage inspection — the primitives the rest of Testkit is built on.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">budget · error · storage</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Dependencies</span><span class="tk-spec__value">soroban-sdk · soroban-env-host · serde · serde_json</span></div>
</div>

## API at a glance

| Type | Purpose |
|------|---------|
| `BudgetSnapshot` | CPU instructions and memory bytes — `last_invocation()` for the call that just ran |
| `BudgetRead` | Trait implemented by any source `capture()` can read: invocation resources, the SDK budget, the host `Budget` |
| `BudgetGuard` | Ceilings and a baseline tolerance for one named operation |
| `BudgetViolation`, `ViolationKind` | The limit a cost broke, rendered as one parseable line |
| `BudgetBaseline` | Recorded costs per case, loaded from and saved to a JSON file |
| `budget_guard!` | Macro form of the guard around a single invocation |
| `DecodedError` | A Soroban error code split into category, meaning and context |
| `ErrorRegistry` | Your `#[contracterror]` codes mapped to the words they mean |
| `ClientOutcome`, `unwrap_decoded` | Unwrap a v28 `try_*` client call, panicking with the decoded error |
| `StorageEntry`, `StorageTier` | A live storage key, its rendered value, tier and expiry ledger |
| `StorageSnapshot` | Every live entry of one contract, captured at a point in time |
| `StorageDiff`, `StorageChange` | Keys added, removed and rewritten between two snapshots |

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

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Scope</span>
  <p>A snapshot covers one contract only: other contracts' data, the WASM <code>ContractCode</code> entry and ledger entries the contract does not own are never included. Account addresses own no contract data, so they capture as empty instead of failing.</p>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Next: <a href="./assert.html">soroban-testkit-assert</a> — turn these measurements into readable assertions.</p>
