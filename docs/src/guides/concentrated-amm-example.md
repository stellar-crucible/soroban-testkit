# Concentrated Liquidity AMM

`examples/concentrated_amm` demonstrates tick-ranged liquidity, swap execution
across active tick bands, fee compounding and testkit budget/storage checks.

Run the example suite:

```bash
cargo test -p soroban-testkit-example-concentrated-amm
```

The contract is `#![no_std]` and exposes `mint_position`, `burn_position`, and
`swap` entry points. Positions carry owner, tick bounds and liquidity. Minting
requires aligned tick spacing; swapping moves the active tick and updates virtual
reserves while fee-bearing output is deducted.

## Test layout

- `fixtures.rs` covers setup, initialization and position lifecycle.
- `assert.rs` covers swaps, tick crossings and authorization failures.
- `budget_storage.rs` captures metering and persistent position behavior.
- `proptest.rs` proves swap reserve invariants and bounded tick math.

The suite uses every workspace testkit crate:

- `soroban-testkit-fixtures` builds isolated environments.
- `soroban-testkit-generators` supplies property-test strategy patterns.
- `soroban-testkit-core` records budget baselines.
- `soroban-testkit-assert` validates wallet and contract behavior.
