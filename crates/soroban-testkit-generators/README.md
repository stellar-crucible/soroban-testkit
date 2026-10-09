# soroban-testkit-generators

proptest strategies that produce Soroban values inside the ranges the chain actually accepts: token amounts, ledger sequences, timestamps.

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies::{ledger_sequence, token_amount};

proptest! {
    #[test]
    fn transfer_never_overflows(amount in token_amount()) {
        // amount: i128 inside a range the chain accepts
    }

    #[test]
    fn ledger_reads_are_bounded(seq in ledger_sequence()) {
        // seq: u32
    }
}
```

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/generators.html>
- Source: [crates/soroban-testkit-generators](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-generators)
- Licence: Apache-2.0
