# Soroban Testkit

A comprehensive testing and debugging toolkit for [Soroban](https://developers.stellar.org/docs/smart-contracts) smart contracts on the Stellar network.

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.91%2B-orange.svg)](https://www.rust-lang.org)
[![Soroban SDK](https://img.shields.io/badge/soroban--sdk-28-purple.svg)](https://docs.rs/soroban-sdk)

## Why Testkit?

Soroban's official SDK provides solid testing primitives, but developers still face daily friction:

- **Verbose test setup** — every test manually creates environments, registers contracts, generates addresses
- **Raw event/auth assertions** — matching on tuples with no ergonomic helpers
- **No budget visibility** — tests pass locally but fail on-chain due to resource exhaustion
- **Heavyweight mocking** — writing full mock contracts just to stub a dependency
- **No property testing primitives** — building Soroban-aware generators from scratch

Testkit fills these gaps with domain-specific tooling that complements (not replaces) the official SDK.

## Crates

| Crate | Description |
|-------|-------------|
| [`testkit-core`](crates/testkit-core) | Budget snapshots, error decoding, storage inspection |
| [`testkit-assert`](crates/testkit-assert) | Ergonomic assertion matchers for events and authorizations |
| [`testkit-fixtures`](crates/testkit-fixtures) | Reusable test context builder and setup framework |
| [`testkit-generators`](crates/testkit-generators) | Soroban-aware property testing strategies for proptest |

## Quick Start

Add to your contract's `Cargo.toml`:

```toml
[dev-dependencies]
testkit-fixtures = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-assert = { git = "https://github.com/stellar-crucible/soroban-testkit" }
```

### Test Fixtures

Eliminate boilerplate with reusable test contexts:

```rust
use testkit_fixtures::builder::TestContextBuilder;

#[test]
fn test_transfer() {
    let ctx = TestContextBuilder::new()
        .with_users(3)
        .build();

    // ctx.env, ctx.admin, ctx.users are ready to use
    let sender = &ctx.users[0];
    let receiver = &ctx.users[1];

    // ... register contract, invoke, assert
}
```

### Event Assertions

Replace raw tuple matching with expressive matchers:

```rust
use testkit_assert::events::EventMatcher;

#[test]
fn test_emits_transfer_event() {
    let env = Env::default();
    env.mock_all_auths();

    // ... invoke contract function

    EventMatcher::new(&env)
        .from_contract(&contract_id)
        .with_topic("Transfer")
        .assert_emitted();
}
```

### Budget Tracking

Catch resource issues before deployment:

```rust
use testkit_core::budget::BudgetSnapshot;

#[test]
fn test_budget_within_limits() {
    let env = Env::default();
    let before = BudgetSnapshot::capture(&env.cost_estimate().budget());

    // ... invoke contract function

    let after = BudgetSnapshot::capture(&env.cost_estimate().budget());
    let diff = before.diff(&after);

    assert!(diff.cpu_insns < 1_000_000, "Function exceeded CPU budget");
}
```

### Property Testing

Generate realistic Soroban values:

```rust
use proptest::prelude::*;
use testkit_generators::strategies;

proptest! {
    #[test]
    fn test_balance_invariants(amount in strategies::token_amount()) {
        // amount is always a valid token balance (0..1e15)
    }
}
```

## Documentation

Full documentation is available at [stellar-crucible.github.io/soroban-testkit](https://stellar-crucible.github.io/soroban-testkit).

## Project Structure

```
soroban-testkit/
├── crates/
│   ├── testkit-core/         # Budget, error, storage utilities
│   ├── testkit-assert/       # Event and auth assertion matchers
│   ├── testkit-fixtures/     # Test context builder and fixtures
│   └── testkit-generators/   # Property testing strategies
├── docs/                     # mdBook documentation source
├── examples/                 # Example contracts using testkit
├── Cargo.toml                # Workspace root
└── LICENSE                   # Apache-2.0
```

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program via [Drips](https://www.drips.network). Check our [issues](https://github.com/stellar-crucible/soroban-testkit/issues) for open tasks with bounty labels.

## License

Licensed under the [Apache License 2.0](LICENSE).
