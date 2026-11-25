# Worked Example: vAMM Perpetual Futures

`examples/vamm` demonstrates synthetic perpetual positions backed by virtual
AMM reserves instead of a spot asset pool. It exercises Testkit fixtures,
assertions, budget/storage inspection and property testing.

## Position model

A position stores the trader, signed size, collateral, entry mark, and
leverage. Positive size is a long and negative size is a short. The mark is
the virtual quote reserve divided by the virtual base reserve.

```rust
let position = client.open_position(&trader, &1, &1_000, &5);
assert_eq!(position.collateral, 1_000);
assert_eq!(position.leverage, 5);
```

Use `move_mark` in tests to simulate an exchange mark change, `unrealized_pnl`
to calculate the live result, and `close_position` to settle and remove the
position.

## Margin and liquidation

The maintenance threshold is 10% of the absolute position notional. A position
is liquidatable when collateral plus unrealized PnL is at or below that threshold.

```rust
if client.is_liquidatable(trader.clone()) {
    client.liquidate(trader);
}
```

The suite verifies that margin additions increase collateral, long and short
PnL have opposite signs around the same mark move, and underwater property cases
always remain eligible for liquidation.

## Funding

`set_funding` and `funding_index` provide a testable funding checkpoint without
introducing a token transfer model. Applications can extend the worked example
with funding accrual and settlement against the index.

## Validation layout

- `tests/fixtures.rs` builds traders and exercises lifecycle inputs.
- `tests/assert.rs` checks authorization, PnL and position removal.
- `tests/budget_storage.rs` measures budgets and storage tiers.
- `tests/property.rs` uses proptest for leverage, liquidation, margin and settlement invariants.
