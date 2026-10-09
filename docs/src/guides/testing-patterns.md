# Testing Patterns

Common patterns for testing Soroban contracts with Testkit: a shared `setup()`, multi-contract interactions, and deterministic ledger time.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Audience</span><span class="tk-spec__value">Contract authors</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Read time</span><span class="tk-spec__value">4 minutes</span></div>
</div>

## Unit test structure

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use soroban_testkit_fixtures::builder::TestContextBuilder;

    fn setup() -> TestContext {
        TestContextBuilder::new()
            .with_users(3)
            .build()
    }

    #[test]
    fn test_happy_path() {
        let ctx = setup();
        // ...
    }

    #[test]
    #[should_panic(expected = "InsufficientBalance")]
    fn test_insufficient_balance() {
        let ctx = setup();
        // ...
    }
}
```

## Cross-contract testing

Register multiple contracts and test interactions:

```rust
#[test]
fn test_contract_interaction() {
    let ctx = TestContextBuilder::new().with_users(2).build();

    let token_id = ctx.env.register(TokenContract, ());
    let vault_id = ctx.env.register(VaultContract, (&token_id,));

    let vault_client = VaultClient::new(&ctx.env, &vault_id);
    vault_client.deposit(&ctx.users[0], &1000);

    // Assert both contracts behaved correctly
}
```

## Time-dependent tests

Manipulate ledger state for time-sensitive logic:

```rust
#[test]
fn test_vesting_unlock() {
    let ctx = TestContextBuilder::new().with_users(1).build();

    ctx.env.ledger().set(LedgerInfo {
        timestamp: 1_700_000_000,
        ..Default::default()
    });

    // ... assert locked

    ctx.env.ledger().set(LedgerInfo {
        timestamp: 1_800_000_000,
        ..Default::default()
    });

    // ... assert unlocked
}
```

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">Reset between cases</span>
  <p>Each <code>TestContext</code> owns its own <code>Env</code>, so ledger writes and storage never leak between tests. Share a <code>setup()</code> helper instead of a mutable global fixture.</p>
</div>

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <h3 class="tk-card__title"><a href="./budget-testing.html">Budget-aware testing</a></h3>
    <p class="tk-card__body">Pin these patterns down with CPU and memory regression assertions.</p>
  </div>
  <div class="tk-card">
    <h3 class="tk-card__title"><a href="./property-testing.html">Property testing</a></h3>
    <p class="tk-card__body">Replace hand-picked inputs with Soroban-aware generators.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <h3 class="tk-card__title"><a href="../crates/assert.html">soroban-testkit-assert</a></h3>
    <p class="tk-card__body">The full matcher API for events and authorizations.</p>
  </div>
</div>
