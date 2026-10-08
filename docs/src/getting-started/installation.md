# Installation

Add the crates you need to your contract's `Cargo.toml`:

```toml
[dev-dependencies]
testkit-core = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-assert = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-fixtures = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-generators = { git = "https://github.com/stellar-crucible/soroban-testkit", features = ["proptest"] }
```

## Prerequisites

- Rust 1.81 or later
- Soroban SDK 22.x
- For property testing: `proptest` or `arbitrary` crate

## Feature Flags

### testkit-generators

| Feature | Description |
|---------|-------------|
| `proptest` (default) | Enable proptest strategies |
| `arbitrary` | Enable arbitrary trait implementations |
