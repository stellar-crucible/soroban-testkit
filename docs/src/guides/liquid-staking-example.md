# Worked Example: Yield-Bearing Liquid Staking

The `examples/liquid_staking` workspace member keeps shares fixed while
staking yield raises the amount of underlying asset represented by each share.
Depositors receive shares at the current exchange rate; redemption burns shares
and transfers the corresponding pro-rata underlying amount. Integer division
uses checked multiplication before division so intermediate overflow is
reported instead of silently wrapping.

## Invariants covered

- Initial deposits mint shares one-to-one with underlying units.
- Yield increases total assets without minting shares, so the exchange rate
  rises over time.
- A redemption removes the proportional share of assets and updates both
  supply and the holder's balance.
- Proptest accrues a positive random yield and verifies redemption value never
  decreases as yield accumulates.

The integration tests are split into fixtures, behavior assertions,
budget/storage checks, and property tests. The exchange-rate gas check uses a
committed CPU ceiling to catch accidental growth in this simple calculation.

Run the example with `cargo test -p soroban-testkit-example-liquid-staking`.
