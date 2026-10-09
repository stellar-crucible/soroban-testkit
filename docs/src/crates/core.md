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
| `StorageEntry`, `StorageTier` | A live storage key, its rendered value, tier and expiry ledger |
| `StorageSnapshot` | Every live entry of one contract, captured at a point in time |
| `StorageDiff`, `StorageChange` | Keys added, removed and rewritten between two snapshots |

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

## Storage snapshots and diffs

Capture everything a contract holds, then compare two captures around an
operation. Keys and values are rendered to stable strings (`u32` becomes `"42"`,
a map becomes `"{a: 1, b: 2}"`), so a diff reports what changed rather than two
opaque blobs. Instance storage is unfolded entry by entry, and each entry
carries the ledger sequence it expires at.

```rust
use testkit_core::storage::{inspect_storage, StorageSnapshot, StorageTier};

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

<p class="tk-muted">Next: <a href="./assert.html">testkit-assert</a> — turn these measurements into readable assertions.</p>
