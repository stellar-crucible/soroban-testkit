# soroban-testkit-core

Budget snapshots, budget guards, error decoding and storage inspection — the primitives the rest of [Soroban Testkit](https://github.com/stellar-crucible/soroban-testkit) is built on.

```rust
use soroban_testkit_core::budget::BudgetGuard;
use soroban_testkit_core::error::{DecodedError, ErrorRegistry};
use soroban_testkit_core::storage::StorageSnapshot;

BudgetGuard::new("increment").cpu_ceiling(20_000_000).run(&env, || client.increment(&caller, &1));
let decoded = DecodedError::from_error_with(&error, &registry);
let diff = before.diff(&StorageSnapshot::capture(&env, &contract));
```

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/core.html>
- Source: [crates/soroban-testkit-core](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-core)
- Licence: Apache-2.0
