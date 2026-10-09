# Budget-Aware Testing

Soroban contracts execute within strict CPU and memory budgets. Tests that pass locally can fail on-chain once resource consumption exceeds the transaction limits — Testkit makes those costs visible in ordinary assertions.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Crate</span><span class="tk-spec__value">soroban-testkit-core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">API</span><span class="tk-spec__value">BudgetSnapshot · diff()</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Runs in</span><span class="tk-spec__value">cargo test / CI</span></div>
</div>

## Why budget matters

The Soroban runtime enforces resource budgets per transaction. The SDK's test environment does not enforce those limits by default, so a contract can pass every unit test and still fail during deployment or execution on testnet and mainnet.

<div class="tk-problems">
  <div class="tk-problem">
    <span class="tk-problem__label">Symptom</span>
    <span class="tk-problem__body">Green test suite, HOST_VALUE_SIZE / budget exhausted error on-chain.</span>
    <span class="tk-problem__fix">Fix: assert on the <b>CPU and memory delta</b> of each call, not just its result.</span>
  </div>
</div>

## Basic budget tracking

```rust
use soroban_testkit_core::budget::BudgetSnapshot;

#[test]
fn test_budget_regression() {
    let env = Env::default();
    env.mock_all_auths();

    let before = BudgetSnapshot::capture(&env.cost_estimate().budget());
    // ... invoke function
    let after = BudgetSnapshot::capture(&env.cost_estimate().budget());
    let cost = before.diff(&after);

    // Set thresholds based on your contract's SLA
    assert!(cost.cpu_insns < 1_000_000, "CPU: {}", cost.cpu_insns);
    assert!(cost.mem_bytes < 100_000, "Memory: {}", cost.mem_bytes);
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Where the numbers come from</span>
  <p>SDK v28 exposes costs through <code>env.cost_estimate().budget()</code>. The older <code>env.budget()</code> accessor is deprecated and should not appear in new tests.</p>
</div>

## Reading a snapshot

| Field | Meaning | Suggested use |
|-------|---------|---------------|
| `cpu_insns` | Instructions consumed between captures | Regression guard in CI |
| `mem_bytes` | Memory footprint between captures | Keeps entries under ledger limits |
| `diff()` | Signed delta of two snapshots | Compare before / after one call |

## CI integration

Track budget trends across commits to catch regressions early:

<ol class="tk-steps">
  <li>
    <h4>Store a baseline</h4>
    <p>Commit the snapshot values for each hot-path function next to the test that produces them.</p>
  </li>
  <li>
    <h4>Fail on drift</h4>
    <p>Assert the delta stays inside a agreed tolerance, so an accidental extra storage read breaks the build instead of the network.</p>
  </li>
  <li>
    <h4>Publish the numbers</h4>
    <p>Print the snapshot in the test output — CI logs become a cost history you can diff between releases.</p>
  </li>
</ol>

<hr class="tk-divider" />

<p class="tk-muted">Related: <a href="./property-testing.html">Property testing</a> for inputs, <a href="../crates/core.html">soroban-testkit-core</a> for the full budget API.</p>
