# Budget-Aware Testing

Soroban contracts execute within strict CPU and memory budgets. Tests that pass locally can fail on-chain once resource consumption exceeds the transaction limits — Testkit makes those costs visible in ordinary assertions.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Crate</span><span class="tk-spec__value">soroban-testkit-core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">API</span><span class="tk-spec__value">BudgetSnapshot · BudgetGuard · BudgetBaseline · budget_guard!</span></div>
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

<div class="tk-callout">
  <span class="tk-callout__title">Warm the estimator first</span>
  <p>An environment that has never run a contract call reports nothing to estimate, so <code>capture()</code> on it reads zero. Make one call — a read is enough — before the first snapshot.</p>
</div>

## Reading a snapshot

| Field | Meaning | Suggested use |
|-------|---------|---------------|
| `cpu_insns` | Instructions consumed between captures | Regression guard in CI |
| `mem_bytes` | Memory footprint between captures | Keeps entries under ledger limits |
| `diff()` | Signed delta of two snapshots | Compare before / after one call |

## Asserting a limit instead of a number

`BudgetGuard` carries the two limits a reviewer can argue about: a ceiling this
call may not cross, and a growth allowance against the cost recorded the last
time the case was measured.

<ol class="tk-steps">
  <li>
    <h4>Name the case</h4>
    <p><code>BudgetGuard::new("transfer")</code>. The name is what every failure line carries, so a CI log points at an operation rather than at a test file.</p>
  </li>
  <li>
    <h4>Set the ceilings</h4>
    <p><code>.cpu_ceiling(2_000_000).mem_ceiling(500_000)</code> — absolute limits, independent of history.</p>
  </li>
  <li>
    <h4>Add the baseline</h4>
    <p><code>.baseline(Some(recorded)).tolerance_percent(10)</code> rejects a call that grew more than 10% past the recorded cost. A tolerance of <code>0</code> means not one instruction more.</p>
  </li>
  <li>
    <h4>Measure the call</h4>
    <p><code>.run(&amp;env, || client.transfer(&amp;from, &amp;to, &amp;1000))</code> captures before and after itself and asserts the delta, returning the invocation's value.</p>
  </li>
</ol>

```rust
use soroban_testkit_core::budget::BudgetGuard;

#[test]
fn increment_stays_inside_its_budget() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (id, client) = register(&ctx);
    let caller = ctx.users[0].clone();

    client.get(); // warm the cost estimator

    BudgetGuard::new("increment")
        .cpu_ceiling(20_000_000)
        .mem_ceiling(20_000_000)
        .run(&ctx.env, || client.increment(&caller, &1));
}
```

The `budget_guard!` macro is the same assertion written around the call:

```rust
use soroban_testkit_core::budget_guard;

budget_guard!(&env, "transfer", {
    cpu_max: 2_000_000,
    mem_max: 500_000,
    baseline: recorded,        // Option<BudgetSnapshot>
    tolerance: 10,             // percent
}, || client.transfer(&sender, &receiver, &1000));
```

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Both readings, once</span>
  <p>A guard reports <b>every</b> limit a cost breaks — ceilings first, then growth — instead of stopping at the first. One run tells you whether a change moved CPU, memory, or both.</p>
</div>

## Committing the numbers: baseline files

A baseline is a JSON file mapping case names to the cost recorded for them, so
the limits live in the repository and a pull request shows a cost change as a
diff.

```json
{
  "version": 1,
  "cases": {
    "increment": { "cpu_insns": 1873412, "mem_bytes": 216480 },
    "get": { "cpu_insns": 244160, "mem_bytes": 30720 }
  }
}
```

```rust
use soroban_testkit_core::budget::BudgetBaseline;

let mut baseline = BudgetBaseline::load(std::path::Path::new("tests/budget.json")).unwrap();

baseline
    .guard("increment")            // carries the recorded cost for the case
    .tolerance_percent(5)
    .run(&env, || client.increment(&caller, &1));

// Re-record and commit the file when a cost change is intended:
baseline.record("increment", measured_cost);
baseline.save(std::path::Path::new("tests/budget.json")).unwrap();
```

`guard()` on a case the file does not know returns a guard with no baseline, so
a new operation is only bounded by whatever ceilings the test adds — the missing
case never fails a suite by itself. A file written by a newer Testkit is refused
rather than half-read.

<div class="tk-problems">
  <div class="tk-problem">
    <span class="tk-problem__label">Symptom</span>
    <span class="tk-problem__body">A baseline that regenerates on every run accepts whatever the last run cost.</span>
    <span class="tk-problem__fix">Fix: commit the file, and rewrite it only in a deliberate "record budgets" change.</span>
  </div>
</div>

## Machine-readable output

Every breach renders as one line of `key=value` pairs, which is what lets a CI
step grep, annotate, or trend them:

```text
BUDGET kind=ceiling case=transfer metric=cpu_insns actual=2500000 limit=2000000
BUDGET kind=growth case=transfer metric=mem_bytes actual=610000 limit=550000 baseline=500000 tolerance_percent=10
```

| Field | Meaning |
|-------|---------|
| `kind` | `ceiling` (absolute limit) or `growth` (past a baseline) |
| `case` | The name the guard was built with |
| `metric` | `cpu_insns` or `mem_bytes` |
| `actual` / `limit` | Measured cost and the largest value still accepted |
| `baseline` / `tolerance_percent` | Only on `growth`, so the ratio is recoverable from the line |

## CI integration

Track budget trends across commits to catch regressions early:

<ol class="tk-steps">
  <li>
    <h4>Store a baseline</h4>
    <p>Commit one `budget.json` per suite, one case per hot-path function, and let <code>guard(case)</code> read it back.</p>
  </li>
  <li>
    <h4>Fail on drift</h4>
    <p>Assert the delta stays inside an agreed tolerance, so an accidental extra storage read breaks the build instead of the network.</p>
  </li>
  <li>
    <h4>Publish the numbers</h4>
    <p>The failure lines are already parseable — print them in the job summary and CI logs become a cost history you can diff between releases.</p>
  </li>
</ol>

<div class="tk-callout">
  <span class="tk-callout__title">Estimator overhead</span>
  <p><code>run()</code> measures around <code>cost_estimate()</code>, so its own work lands in the delta. It is small, fixed, and present in the baseline too — the two readings cancel, which is why a recorded cost is a better limit than a hand-written guess.</p>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Related: <a href="./property-testing.html">Property testing</a> for inputs, <a href="../crates/core.html">soroban-testkit-core</a> for the full budget API.</p>
