# testkit-core

Core utilities for budget tracking, error decoding, and storage inspection.

## Budget Snapshots

Capture and compare CPU/memory consumption between operations:

```rust
use testkit_core::budget::BudgetSnapshot;

let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
// ... invoke contract
let after = BudgetSnapshot::capture(&env.cost_estimate().budget());
let diff = before.diff(&after);

println!("CPU instructions: {}", diff.cpu_insns);
println!("Memory bytes: {}", diff.mem_bytes);
```

## Error Decoding

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

## Storage Inspection

Examine contract storage entries and their tiers:

```rust
use testkit_core::storage::{inspect_storage, StorageTier};

let entries = inspect_storage(&env, &contract_id);
for entry in &entries {
    println!("{:?} storage: {}", entry.tier, entry.key);
}
```
