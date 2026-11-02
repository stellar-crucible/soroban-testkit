# Property Testing

Property-based testing checks invariants across hundreds of generated inputs, catching the edge cases example-based tests miss. Testkit's generators keep those inputs inside ranges Soroban actually accepts.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Crate</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Backend</span><span class="tk-spec__value">proptest 1.x</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Cases</span><span class="tk-spec__value">256 by default</span></div>
</div>

## Setup

Enable the `proptest` feature:

```toml
[dev-dependencies]
soroban-testkit-generators = { version = "0.3.0", features = ["proptest"] }
proptest = "1"
```

## Basic property test

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn balance_never_negative(amount in strategies::token_amount()) {
        // All generated amounts are >= 0 and within realistic bounds
        assert!(amount >= 0);
    }
}
```

## Combining with fixtures

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

<div class="tk-callout">
  <span class="tk-callout__title">Mind the host</span>
  <p>Every generated case builds a real <code>Env</code> and executes on the host. If the suite slows down, reduce cases with <code>#![proptest_config(ProptestConfig::with_cases(64))]</code> before you narrow the strategy ranges.</p>
</div>

## Generating principals

Access-control properties need callers, and a caller in Soroban is an address rather than a string. `address()` generates a valid 32-byte principal in either format, `account_address()` and `contract_address()` pin one, and `address_of_type(AddressType::Account)` takes the format as an argument. Each yields an `AddressBytes`, which becomes a host value through `to_address(&env)`.

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn only_the_caller_moves_their_balance(
        caller in strategies::account_address(),
        amount in strategies::token_amount(),
    ) {
        let ctx = TestContextBuilder::new().build();
        let caller = caller.to_address(&ctx.env);
        // ... register the contract, fund `caller`, then assert that the same
        //     call from a different address fails or records no authorization
    }
}
```

Generating the *contract* form is what catches an address-format assumption — a `C...` id where the code only ever saw a `G...` account. The full list with return types is in the [strategy reference](../crates/generators.html).

<div class="tk-callout">
  <span class="tk-callout__title">Uniqueness is per run, not per seed</span>
  <p>Every generated address carries a process-wide nonce, so two calls in one test run never collide even when their random prefixes do. The trade-off is that a value depends on generation order: judge a failing case by the bytes proptest prints rather than by replaying a seed alone.</p>
</div>

## When to use property testing

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Invariants</span>
    <h3 class="tk-card__title">Arithmetic</h3>
    <p class="tk-card__body">Supply conservation, no overflow, and rounding that stays consistent across amounts.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Invariants</span>
    <h3 class="tk-card__title">State machines</h3>
    <p class="tk-card__body">A valid state remains reachable after any sequence of operations, not just the one you scripted.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Security</span>
    <h3 class="tk-card__title">Access control</h3>
    <p class="tk-card__body">Unauthorized callers fail regardless of the parameters they present.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Edges</span>
    <h3 class="tk-card__title">Boundaries</h3>
    <p class="tk-card__body">Zero amounts, maximum values and empty collections are generated rather than remembered.</p>
  </div>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Strategy reference: <a href="../crates/generators.html">soroban-testkit-generators</a>.</p>
