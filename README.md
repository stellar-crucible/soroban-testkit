# Soroban Testkit

A comprehensive testing and debugging toolkit for [Soroban](https://developers.stellar.org/docs/smart-contracts) smart contracts on the Stellar network.

**📖 Documentation: [stellar-crucible.github.io/soroban-testkit](https://stellar-crucible.github.io/soroban-testkit/)**

[![Docs](https://img.shields.io/badge/docs-mdBook-1a2f6b.svg)](https://stellar-crucible.github.io/soroban-testkit/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.91%2B-orange.svg)](https://www.rust-lang.org)
[![Soroban SDK](https://img.shields.io/badge/soroban--sdk-28-purple.svg)](https://docs.rs/soroban-sdk)
[![CI](https://github.com/stellar-crucible/soroban-testkit/actions/workflows/ci.yml/badge.svg)](https://github.com/stellar-crucible/soroban-testkit/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/stellar-crucible/soroban-testkit?label=release&color=brightgreen)](https://github.com/stellar-crucible/soroban-testkit/releases/latest)
[![crates.io](https://img.shields.io/crates/v/soroban-testkit-core?label=crates.io&color=f79071)](https://crates.io/crates/soroban-testkit-core)
[![Open tasks](https://img.shields.io/github/issues/stellar-crucible/soroban-testkit/Stellar%20Wave?label=Stellar%20Wave%20tasks&color=5319E7)](https://github.com/stellar-crucible/soroban-testkit/issues?q=is%3Aopen+is%3Aissue+label%3A%22Stellar+Wave%22)

## Documentation

| Guide | Read it |
|-------|---------|
| Introduction & architecture | [Docs home](https://stellar-crucible.github.io/soroban-testkit/) |
| Installation | [Getting started](https://stellar-crucible.github.io/soroban-testkit/getting-started/installation.html) |
| First test in 5 minutes | [Quick start](https://stellar-crucible.github.io/soroban-testkit/getting-started/quick-start.html) |
| Crate references | [core](https://stellar-crucible.github.io/soroban-testkit/crates/core.html) · [assert](https://stellar-crucible.github.io/soroban-testkit/crates/assert.html) · [fixtures](https://stellar-crucible.github.io/soroban-testkit/crates/fixtures.html) · [generators](https://stellar-crucible.github.io/soroban-testkit/crates/generators.html) |
| Recipes | [Testing patterns](https://stellar-crucible.github.io/soroban-testkit/guides/testing-patterns.html) · [Worked example: counter](https://stellar-crucible.github.io/soroban-testkit/guides/counter-example.html) · [Budget-aware testing](https://stellar-crucible.github.io/soroban-testkit/guides/budget-testing.html) · [Property testing](https://stellar-crucible.github.io/soroban-testkit/guides/property-testing.html) |
| Contribute & earn | [Contributing guide](https://stellar-crucible.github.io/soroban-testkit/contributing.html) · [Roadmap](https://stellar-crucible.github.io/soroban-testkit/roadmap.html) |

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
| [`soroban-testkit-core`](crates/soroban-testkit-core) | Budget snapshots, error decoding, storage inspection |
| [`soroban-testkit-assert`](crates/soroban-testkit-assert) | Ergonomic assertion matchers for events and authorizations |
| [`soroban-testkit-fixtures`](crates/soroban-testkit-fixtures) | Reusable test context builder and setup framework |
| [`soroban-testkit-generators`](crates/soroban-testkit-generators) | Soroban-aware property testing strategies for proptest |

## Quick Start

Add to your contract's `Cargo.toml`:

```toml
[dev-dependencies]
soroban-testkit-fixtures = "0.2.0"
soroban-testkit-assert = "0.2.0"
```

Prefer pinning the source? Use the release tag, which always points at a commit whose CI is green:

```toml
[dev-dependencies]
soroban-testkit-fixtures = { git = "https://github.com/stellar-crucible/soroban-testkit", tag = "v0.2.0" }
soroban-testkit-assert = { git = "https://github.com/stellar-crucible/soroban-testkit", tag = "v0.2.0" }
```

### Test Fixtures

Eliminate boilerplate with reusable test contexts:

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;

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
use soroban_testkit_assert::events::EventMatcher;

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
use soroban_testkit_core::budget::BudgetGuard;

#[test]
fn test_budget_within_limits() {
    BudgetGuard::new("transfer")
        .cpu_ceiling(1_000_000)
        .mem_ceiling(100_000)
        .baseline(recorded_cost)      // from a committed JSON file
        .tolerance_percent(10)
        .run(&env, || client.transfer(&sender, &receiver, &1000));
}
```

A call that crosses a ceiling or grows past its recorded cost fails the test with
one parseable line per breach, so a budget regression shows up in CI as a diff
rather than as a surprise on mainnet.

### Property Testing

Generate realistic Soroban values:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn test_balance_invariants(amount in strategies::token_amount()) {
        // amount is always a valid token balance (0..1e15)
    }
}
```

## Project Structure

```
soroban-testkit/
├── crates/
│   ├── soroban-testkit-core/         # Budget, error, storage utilities
│   ├── soroban-testkit-assert/       # Event and auth assertion matchers
│   ├── soroban-testkit-fixtures/     # Test context builder and fixtures
│   └── soroban-testkit-generators/   # Property testing strategies
├── examples/
│   └── counter/              # Contract + integration tests using every crate
├── docs/                     # mdBook documentation source (theme/custom.css)
├── .github/                  # CI, docs deploy, templates, CODEOWNERS
├── Cargo.toml                # Workspace root
└── LICENSE                   # Apache-2.0
```

## Development

```bash
cargo test --workspace --locked      # 130 tests: all four crates + examples/counter
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
mdbook build docs                    # documentation site
cargo deny check all                 # advisories, licenses, duplicate versions, sources
```

[`examples/counter`](examples/counter) is a real contract with a test suite that
uses every crate in the workspace: fixtures for the environment and users,
`EventMatcher` for contract events, `BudgetGuard` and `budget_guard!` for CPU and
memory ceilings, and a `#[should_panic]` case that shows an unmocked
`require_auth` failing.

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program via [Drips](https://www.drips.network). Check our [issues](https://github.com/stellar-crucible/soroban-testkit/issues) for open tasks with bounty labels.

## License

Licensed under the [Apache License 2.0](LICENSE).
