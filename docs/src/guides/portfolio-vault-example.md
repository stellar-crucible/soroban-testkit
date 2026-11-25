# Worked Example: Portfolio Rebalancing Vault

This guide walks through `examples/portfolio_vault`, a multi-asset index vault
that mints portfolio shares, tracks oracle prices, and executes simplified
oracle-priced rebalancing swaps.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/portfolio_vault</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">30+ across 4 files</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-portfolio-vault</span></div>
</div>

## Contract model

`PortfolioVault` stores target asset weights (basis points), oracle prices, and
user share balances. Deposits mint shares proportional to contributed value;
withdrawals burn shares pro-rata across vault assets; rebalance executes an
oracle-priced swap between two vault assets with slippage protection.

## Four-file test split

| File | Crate | Focus |
|------|-------|-------|
| `tests/fixtures.rs` | `soroban-testkit-fixtures` | Deployment, deposits, withdrawals, oracle updates, rebalance setup |
| `tests/assert.rs` | `soroban-testkit-assert` | Auth/errors and invalid-input rejection |
| `tests/budget_storage.rs` | `soroban-testkit-core` | Budget snapshots, storage snapshots, value preservation checks |
| `tests/proptest.rs` | `soroban-testkit-generators` + `proptest` | Property invariants for deposits, withdrawals, rebalance value/tolerance |

## Fixtures

`tests/common/mod.rs` deploys three Stellar asset contracts, registers the
vault, initializes 50/30/20 target weights, and sets initial oracle prices.

## Assert

Auth and error tests verify admin-only price updates/rebalance, invalid inputs,
and insufficient share withdrawals.

## Core budget/storage

Budget tests capture `BudgetSnapshot::last_invocation` for deposit/withdraw/
rebalance. Storage tests inspect vault snapshots and verify share balance cleanup
and value preservation across oracle-priced rebalance.

## Property tests

Proptest cases verify minted shares are positive, withdrawals cannot exceed
shares, oracle-priced rebalance preserves total value, and single-depositor share
accounting is consistent.

## Running it

```sh
cargo test -p soroban-testkit-example-portfolio-vault
cargo clippy -p soroban-testkit-example-portfolio-vault --all-targets -- -D warnings
```
