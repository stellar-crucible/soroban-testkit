# Budget-Aware Testing

Soroban contracts execute within strict CPU and memory budgets. Tests that pass locally can fail on-chain if resource consumption exceeds limits. Testkit makes budget costs visible.

## Why Budget Matters

The Soroban runtime enforces resource budgets per transaction. The SDK's test environment does not enforce these limits by default, so a contract can pass all unit tests and still fail during deployment or execution on testnet/mainnet.

## Basic Budget Tracking

```rust
use testkit_core::budget::BudgetSnapshot;

#[test]
fn test_budget_regression() {
    let env = Env::default();
    env.mock_all_auths();

    let before = BudgetSnapshot::capture(&env.budget());
    // ... invoke function
    let after = BudgetSnapshot::capture(&env.budget());
    let cost = before.diff(&after);

    // Set thresholds based on your contract's SLA
    assert!(cost.cpu_insns < 1_000_000, "CPU: {}", cost.cpu_insns);
    assert!(cost.mem_bytes < 100_000, "Memory: {}", cost.mem_bytes);
}
```

## CI Integration

Track budget trends across commits to catch regressions early. Log budget snapshots in CI and compare against baselines stored in the repository.
