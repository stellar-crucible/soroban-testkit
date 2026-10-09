# Roadmap

Where Soroban Testkit is going, and which parts are open for contributors. Anything listed here is a proposal until an issue exists for it — the tracker is the source of truth.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Current</span><span class="tk-spec__value">v0.2.0</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Cadence</span><span class="tk-spec__value">Monthly Wave cycle</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tracking</span><span class="tk-spec__value">GitHub Issues</span></div>
</div>

## Shipped in v0.1.0

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-fixtures</span>
    <h3 class="tk-card__title">Test context + builder</h3>
    <p class="tk-card__body">One call to get a mocked <code>Env</code>, an admin address and pre-generated users.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-assert</span>
    <h3 class="tk-card__title">Event and auth matchers</h3>
    <p class="tk-card__body">Chainable <code>assert_emitted</code>, <code>assert_count</code>, <code>assert_no_auth_required</code>, with contract and topic filtering.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-core</span>
    <h3 class="tk-card__title">Budget snapshots</h3>
    <p class="tk-card__body">Capture and diff CPU and memory consumption around a call.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-generators</span>
    <h3 class="tk-card__title">proptest strategies</h3>
    <p class="tk-card__body">Token amounts, ledger sequences and timestamps bounded to realistic ranges.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Quality</span>
    <h3 class="tk-card__title">Test suite for the toolkit</h3>
    <p class="tk-card__body">Unit tests in all four crates, plus <code>cargo fmt</code>, <code>clippy -D warnings</code>, docs build and a <code>cargo deny</code> supply-chain gate in CI.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">examples/counter</span>
    <h3 class="tk-card__title">Worked integration example</h3>
    <p class="tk-card__body">A real Soroban contract tested with fixtures, event matchers, budget assertions and authorization failures.</p>
  </div>
</div>

## Landed since v0.1.0

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-core</span>
    <h3 class="tk-card__title">Storage snapshots and diffs</h3>
    <p class="tk-card__body"><code>StorageSnapshot::capture</code> enumerates a contract's live instance, persistent and temporary entries from the ledger snapshot, and <code>diff()</code> reports added, removed and rewritten keys — <a href="https://github.com/stellar-crucible/soroban-testkit/issues/4">#4</a>, closed.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-assert</span>
    <h3 class="tk-card__title">Negative event assertions</h3>
    <p class="tk-card__body"><code>assert_not_emitted()</code> and <code>assert_none_match()</code> prove a topic stayed silent, quoting the unexpected event when they fail — <a href="https://github.com/stellar-crucible/soroban-testkit/issues/11">#11</a>, closed.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-core</span>
    <h3 class="tk-card__title">Error code decoder</h3>
    <p class="tk-card__body"><code>DecodedError::from_error</code> names the host category and meaning behind a packed error code, and <code>ErrorRegistry</code> adds words for your own <code>#[contracterror]</code> codes — <a href="https://github.com/stellar-crucible/soroban-testkit/issues/5">#5</a>, closed.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-fixtures</span>
    <h3 class="tk-card__title">Ledger time helpers</h3>
    <p class="tk-card__body"><code>advance_time()</code>, <code>set_timestamp()</code> and <code>advance_ledger()</code> move the clock or the height on their own, so a vesting or TTL test reads as a span instead of as a hand-built <code>LedgerInfo</code> — <a href="https://github.com/stellar-crucible/soroban-testkit/issues/12">#12</a>, closed.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-fixtures</span>
    <h3 class="tk-card__title">Context reset</h3>
    <p class="tk-card__body"><code>reset()</code> swaps in a fresh env — ledger, events, auths and storage gone — and carries every address across, so a two-phase test keeps its actors instead of rebuilding its fixture — <a href="https://github.com/stellar-crucible/soroban-testkit/issues/17">#17</a>, closed.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">soroban-testkit-core</span>
    <h3 class="tk-card__title">Budget regression detection</h3>
    <p class="tk-card__body"><code>BudgetGuard</code> and <code>budget_guard!</code> hold a call to a CPU and memory ceiling plus a growth allowance against a recorded cost, and <code>BudgetBaseline</code> keeps those costs in a committed JSON file — every breach printed as one parseable line — <a href="https://github.com/stellar-crucible/soroban-testkit/issues/3">#3</a>, closed.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Distribution</span>
    <h3 class="tk-card__title">On crates.io</h3>
    <p class="tk-card__body">The workspace publishes as <code>soroban-testkit-core</code>, <code>-assert</code>, <code>-fixtures</code> and <code>-generators</code>, so a consumer adds a version requirement instead of a Git pin.</p>
  </div>
</div>

## Next cycle

| Area | Work | Complexity |
|------|------|------------|
| Assertions | `assert_event_emitted!` macro with topic and data matching ([#1](https://github.com/stellar-crucible/soroban-testkit/issues/1)) | `complexity:high` |
| Assertions | `assert_auth_matches!` macro ([#2](https://github.com/stellar-crucible/soroban-testkit/issues/2)) | `complexity:medium` |
| Assertions | Data predicates on emitted events ([#18](https://github.com/stellar-crucible/soroban-testkit/issues/18)) | `complexity:medium` |
| Assertions | Matchers that aggregate events across several invocations ([#19](https://github.com/stellar-crucible/soroban-testkit/issues/19)) | `complexity:medium` |
| Fixtures | Contract registration in the builder ([#6](https://github.com/stellar-crucible/soroban-testkit/issues/6)) | `complexity:medium` |
| Generators | `Address` strategies ([#7](https://github.com/stellar-crucible/soroban-testkit/issues/7)) and XDR-compatible values ([#8](https://github.com/stellar-crucible/soroban-testkit/issues/8)) | `complexity:medium` / `high` |
| Generators | `Arbitrary` implementations for Soroban types ([#16](https://github.com/stellar-crucible/soroban-testkit/issues/16)) | `complexity:medium` |
| Examples | Token example exercising every crate ([#9](https://github.com/stellar-crucible/soroban-testkit/issues/9)) | `complexity:medium` |
| Tooling | Baseline files diffed between PRs in CI ([#14](https://github.com/stellar-crucible/soroban-testkit/issues/14)) | `complexity:medium` |

## Later

<div class="tk-problems">
  <div class="tk-problem">
    <span class="tk-problem__label">Mocking</span>
    <span class="tk-problem__body">Lightweight mock contract generation from trait definitions, so stubbing a dependency stops requiring a whole crate.</span>
    <span class="tk-problem__fix">Tracked as <b>issue #10</b> · <span class="tk-badge tk-badge--brand">High 200 pts</span></span>
  </div>
</div>

## Non-goals

<div class="tk-callout tk-callout--warn">
  <span class="tk-callout__title">What Testkit will not do</span>
  <p>It will not replace the official SDK's testing primitives, will not become a network simulator or transaction builder, and will not fork <code>soroban-env-host</code>. Testkit stays a thin, composable layer on top of the SDK.</p>
</div>

## Shaping the roadmap

Roadmap items start life as issues. If you want a feature that is not here, open a discussion in the issue tracker; if it is accepted and labelled with a complexity tag, it enters the next Wave cycle.

<div class="tk-cta">
  <p>Everything above is claimable once labelled. Pick something that matches the time you have.</p>
  <a class="tk-btn tk-btn--primary" href="https://github.com/stellar-crucible/soroban-testkit/issues">Open the tracker</a>
</div>
