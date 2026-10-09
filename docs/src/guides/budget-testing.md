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
    <span class="tk-problem__fix">Fix: assert on the <b>CPU and memory cost</b> of each call, not just its result.</span>
  </div>
</div>

## Basic budget tracking

The SDK meters one call at a time, and the metering of the call that just
finished is what a test should read:

```rust
use soroban_testkit_core::budget::BudgetSnapshot;

#[test]
fn test_budget_regression() {
    let env = Env::default();
    env.mock_all_auths();

    // ... invoke function
    client.increment(&caller, &1);

    let cost = BudgetSnapshot::last_invocation(&env);
    assert!(cost.cpu_insns < 1_000_000, "CPU: {}", cost.cpu_insns);
    assert!(cost.mem_bytes < 100_000, "Memory: {}", cost.mem_bytes);
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Where the numbers come from</span>
  <p>SDK v28 meters each top-level invocation separately: <code>env.cost_estimate().resources()</code> returns what the call that just ran consumed, and <code>env.cost_estimate().budget()</code> returns the same metering as a running total for that call. The older <code>env.budget()</code> accessor is deprecated and should not appear in new tests.</p>
</div>

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">What a native test contract hides</span>
  <p>A contract registered with <code>env.register(...)</code> runs as host code, so VM instantiation, wasm execution and rent reads are never metered — the reading is the storage and host-work half of the real cost, not all of it. Treat these numbers as a comparison between builds of the same contract rather than as a fee quote, and use a deployed wasm contract when you need the full figure.</p>
</div>

## Reading a snapshot

| Field | Meaning | Suggested use |
|-------|---------|---------------|
| `cpu_insns` | Instructions metered for one invocation | Regression guard in CI |
| `mem_bytes` | Memory metered for one invocation | Keeps entries under ledger limits |
| `last_invocation()` | Snapshot of the call that just ran | The reading a test asserts on |
| `diff()` | Saturating delta of two snapshots | Comparing two readings of one running budget |

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
    <p><code>.run(&amp;env, || client.transfer(&amp;from, &amp;to, &amp;1000))</code> makes the call, reads the metering that call left behind, asserts against it, and returns the invocation's value.</p>
  </li>
</ol>

```rust
use soroban_testkit_core::budget::BudgetGuard;

#[test]
fn increment_stays_inside_its_budget() {
    let ctx = TestContextBuilder::new().with_users(1).build();
    let (id, client) = register(&ctx);
    let caller = ctx.users[0].clone();

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
    "get": { "cpu_insns": 7861, "mem_bytes": 1510 },
    "increment": { "cpu_insns": 32669, "mem_bytes": 5252 }
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

## Running the check in CI

The counter example ships the whole loop: `examples/counter/budget.json` records
the cost of its two hot paths, and two `#[ignore]`d tests in
`examples/counter/src/test.rs` drive it.

```bash
# record the costs where they will be enforced
cargo test -p soroban-testkit-example-counter -- --ignored --nocapture \
    budget_baseline_records_current_costs

# compare a run against the committed file
TESTKIT_BUDGET_TOLERANCE=5 cargo test -p soroban-testkit-example-counter -- --ignored --nocapture \
    budget_baseline_rejects_drifted_costs
```

Both are ignored on purpose — a cost belongs to the platform that measured it, so an ordinary `cargo test --workspace` run stays green on any machine. The `Budget baseline` workflow runs the check on one pinned `ubuntu-latest` runner, prints the numbers in the job summary, comments them on the pull request, and fails the job when a case grows more than the tolerance: 10% by default, `tolerance` on a manual run, `fail_on_drift` off when you want the report without the verdict.

```text
BUDGET_SUMMARY case=get metric=cpu_insns actual=7861 baseline=7861 drift=+0.00%
BUDGET_SUMMARY case=increment metric=cpu_insns actual=32669 baseline=32669 drift=+0.00%
```

| Field | Meaning |
|-------|---------|
| `case` | The key in the committed `budget.json` |
| `actual` | What the call cost on this runner |
| `baseline` | What the committed file says it cost |
| `drift` | `actual` against `baseline`, signed percent |

A breach then follows as the usual `BUDGET kind=growth …` line, so the same grep
covers both the check and an in-test guard.

<ol class="tk-steps">
  <li>
    <h4>An unintended rise</h4>
    <p>The job fails and the comment says which case, which metric and by how much. Nothing to record — the code is what has to change.</p>
  </li>
  <li>
    <h4>An intended rise</h4>
    <p>Run the workflow with <b>Record</b> checked: it rewrites <code>budget.json</code> on the same runner that enforces it and uploads the file as an artifact. Commit that file on the branch, and the pull request diff reads as a reviewable statement — <code>increment: 32669 → 41000</code> — rather than a red check someone retried.</p>
  </li>
</ol>

<div class="tk-callout">
  <span class="tk-callout__title">One guard, one call</span>
  <p>The metering the SDK reports belongs to the top-level invocation that has just finished, so a closure that makes two calls is judged on the second one. Give each call its own guard, and name the case after the call you care about.</p>
</div>

<hr class="tk-divider" />

<p class="tk-muted">Related: <a href="./property-testing.html">Property testing</a> for inputs, <a href="../crates/core.html">soroban-testkit-core</a> for the full budget API.</p>
