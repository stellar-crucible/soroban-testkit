# soroban-testkit-generators

Soroban-aware property-testing strategies for `proptest` and `arbitrary`, so generated inputs respect the ranges real contracts accept.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Features</span><span class="tk-spec__value">proptest (default) · arbitrary</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Module</span><span class="tk-spec__value">strategies</span></div>
</div>

## Available strategies

| Strategy | Type | Range |
|----------|------|-------|
| `token_amount()` | `i128` | 0 to 1e15 |
| `ledger_sequence()` | `u32` | 1 to 10,000,000 |
| `timestamp()` | `u64` | Sep 2020 to May 2033 |

## Usage with proptest

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn transfer_preserves_total_supply(
        amount in strategies::token_amount(),
        seq in strategies::ledger_sequence(),
    ) {
        // amount is always a valid token balance
        // seq is always a realistic ledger sequence number
    }
}
```

## Custom strategies

Compose the built-ins with proptest combinators:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

fn transfer_args() -> impl Strategy<Value = (i128, i128)> {
    (strategies::token_amount(), strategies::token_amount())
        .prop_filter("sender must have sufficient balance", |(a, b)| a >= b)
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Keep cases bounded</span>
  <p>Soroban tests run on the host and pay for every instruction. Start <code>proptest!</code> cases at the default 256, and shrink ranges before raising the count — <a href="../guides/property-testing.html">Property Testing</a> covers the trade-offs.</p>
</div>

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Feature flags</span>
    <h3 class="tk-card__title">Opt in per backend</h3>
    <p class="tk-card__body">Enable <code>proptest</code>, <code>arbitrary</code>, or both. Unused backends stay out of your dependency graph.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Good first issue</span>
    <h3 class="tk-card__title">More strategies</h3>
    <p class="tk-card__body">Address, symbol and <code>BytesN&lt;32&gt;</code> generators are labelled <span class="tk-badge tk-badge--brand">Medium 150 pts</span> on the issue tracker.</p>
  </div>
</div>
