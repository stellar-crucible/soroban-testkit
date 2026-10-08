# Soroban Testkit

**Soroban Testkit** is an open-source testing and debugging framework for [Soroban](https://developers.stellar.org/docs/smart-contracts) smart contracts on the Stellar network.

It provides ergonomic utilities that complement the official Soroban SDK, helping developers write reliable smart contracts with confidence.

## What It Solves

The Soroban SDK provides solid testing primitives (`Env::default()`, `testutils`, event inspection), but developers still encounter daily friction:

- **Verbose test setup** — every test manually creates environments, registers contracts, generates addresses, and seeds state
- **Raw assertions** — matching events and authorizations requires manual tuple destructuring with no domain-specific helpers
- **Hidden resource costs** — tests pass locally but fail on-chain due to CPU/memory budget exhaustion
- **Heavyweight mocking** — stubbing a dependency requires writing an entire mock contract
- **No property testing support** — building Soroban-aware generators from scratch for every project

## Architecture

Testkit is organized as a Cargo workspace with four focused crates:

```
testkit-core        → Budget snapshots, error decoding, storage inspection
testkit-assert      → Ergonomic event and authorization matchers
testkit-fixtures    → Reusable test context builder (Foundry-style setUp)
testkit-generators  → proptest/arbitrary strategies for Soroban types
```

Each crate can be used independently or combined for a complete testing experience.

## Stellar Wave

This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program via [Drips Network](https://www.drips.network). Contributors earn rewards by resolving issues tagged with complexity labels. See our [GitHub Issues](https://github.com/stellar-crucible/soroban-testkit/issues) for available tasks.
