# Testing Patterns

Common patterns for testing Soroban contracts with Testkit.

## Unit Test Structure

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use testkit_fixtures::builder::TestContextBuilder;

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

## Cross-Contract Testing

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

## Time-Dependent Tests

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
