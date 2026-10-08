# Property Testing

Property-based testing verifies invariants across many randomly generated inputs, catching edge cases that example-based tests miss.

## Setup

Enable the `proptest` feature:

```toml
[dev-dependencies]
testkit-generators = { git = "https://github.com/stellar-crucible/soroban-testkit", features = ["proptest"] }
proptest = "1"
```

## Basic Property Test

```rust
use proptest::prelude::*;
use testkit_generators::strategies;

proptest! {
    #[test]
    fn balance_never_negative(amount in strategies::token_amount()) {
        // All generated amounts are >= 0 and within realistic bounds
        assert!(amount >= 0);
    }
}
```

## Combining with Fixtures

```rust
proptest! {
    #[test]
    fn transfer_preserves_supply(
        amount_a in strategies::token_amount(),
        amount_b in strategies::token_amount(),
    ) {
        let ctx = TestContextBuilder::new().with_users(2).build();
        // ... initialize balances, perform transfers
        // ... assert total supply unchanged
    }
}
```

## When to Use Property Testing

- Arithmetic invariants (supply conservation, no overflow)
- State machine transitions (valid state after any sequence of operations)
- Access control (unauthorized calls always fail regardless of parameters)
- Boundary conditions (zero amounts, max values, empty collections)
