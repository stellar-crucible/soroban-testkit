# testkit-core

Budget tracking, error decoding and storage inspection — the primitives the rest of Testkit is built on.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">testkit-core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">budget · error · storage</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Dependencies</span><span class="tk-spec__value">soroban-sdk</span></div>
</div>

## API at a glance

| Type | Purpose |
|------|---------|
| `BudgetSnapshot` | Capture CPU instructions and memory bytes at a point in time |
| `BudgetRead` | Trait implemented by any budget `capture()` can read — host `Budget` and the SDK budget |
| `DecodedError` | A Soroban error code rendered with message and context |
| `StorageEntry`, `StorageTier` | A storage key plus its instance / persistent / temporary tier |

## Budget snapshots

Capture and compare CPU and memory consumption around a single operation.
`capture()` takes anything implementing `BudgetRead`, so the same snapshot works
with `env.cost_estimate().budget()` in tests and with a
`soroban_env_host::budget::Budget` you charge by hand. `diff()` saturates at
zero, so a later snapshot that reads smaller never underflows an assertion.

```rust
use testkit_core::budget::BudgetSnapshot;

let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
// ... invoke contract
let after = BudgetSnapshot::capture(&env.cost_estimate().budget());
let diff = before.diff(&after);

println!("CPU instructions: {}", diff.cpu_insns);
println!("Memory bytes: {}", diff.mem_bytes);
```

<div class="tk-callout">
  <span class="tk-callout__title">SDK v28 note</span>
  <p><code>env.budget()</code> is deprecated. Always go through <code>env.cost_estimate().budget()</code> so the snapshot reflects the current cost model.</p>
</div>

## Error decoding

Translate opaque Soroban error codes into human-readable messages:

```rust
use testkit_core::error::DecodedError;

let err = DecodedError {
    code: 12,
    message: "Insufficient balance".to_string(),
    context: Some("transfer".to_string()),
};
println!("{}", err); // "Soroban Error [12]: Insufficient balance (context: transfer)"
```

`DecodedError` implements `Display`, so it reads well inside `assert!` messages and test output.

## Storage inspection

Examine contract storage entries and the tier each one lives in:

```rust
use testkit_core::storage::{inspect_storage, StorageTier};

let entries = inspect_storage(&env, &contract_id);
for entry in &entries {
    println!("{:?} storage: {}", entry.tier, entry.key);
}
```

<div class="tk-callout tk-callout--warn">
  <span class="tk-callout__title">Work in progress</span>
  <p><code>inspect_storage</code> currently returns a best-effort view. Enumerating every live key precisely across tiers is an open <a href="https://github.com/stellar-crucible/soroban-testkit/issues">wave issue</a> — a good place to start contributing.</p>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Next: <a href="./assert.html">testkit-assert</a> — turn these measurements into readable assertions.</p>
