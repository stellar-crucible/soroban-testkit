# testkit-assert

Fluent matchers for contract events and authorizations, so assertions read like the behaviour you expect.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">testkit-assert</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">events · auth</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Style</span><span class="tk-spec__value">chainable builder</span></div>
</div>

## API at a glance

| Method | Asserts |
|--------|---------|
| `EventMatcher::assert_emitted()` | At least one matching event exists |
| `EventMatcher::assert_count(n)` | Exactly `n` events were emitted |
| `EventMatcher::from_contract(addr)` | Restricts matches to one contract address |
| `EventMatcher::with_topic(t)` | Restricts matches to events carrying that topic symbol |
| `AuthMatcher::assert_no_auth_required()` | The call needed no authorizations |
| `AuthMatcher::assert_auth_count(n)` | Exactly `n` authorizations were recorded |

## Event matching

Replace raw tuple iteration with a fluent API:

```rust
use testkit_assert::events::EventMatcher;

// Assert at least one event was emitted
EventMatcher::new(&env).assert_emitted();

// Assert exact count
EventMatcher::new(&env).assert_count(3);

// Filter by contract and topic
EventMatcher::new(&env)
    .from_contract(&contract_id)
    .with_topic("Transfer")
    .assert_emitted();
```

<div class="tk-callout tk-callout--warn">
  <span class="tk-callout__title">Scope: the latest invocation</span>
  <p><code>EventMatcher</code> reads <code>env.events().all()</code>, and in SDK v28 that returns only the events published by the <strong>most recent contract invocation</strong>. Assert right after each call instead of accumulating counts over a test; a matcher created after a second call sees the second call alone. <a href="https://github.com/stellar-crucible/soroban-testkit/issues/19">Issue #19</a> tracks matchers that aggregate across invocations.</p>
</div>

<div class="tk-callout">
  <span class="tk-callout__title">Reading events in v28</span>
  <p><code>env.events().all()</code> now returns <code>ContractEvents</code>. Call <code>.events()</code> on it to get the slice, and import <code>soroban_sdk::testutils::Events as _</code> — the method is feature-gated behind <code>testutils</code>. Testkit applies the contract and topic filters for you.</p>
</div>

## Authorization matching

Verify who had to sign for a call:

```rust
use testkit_assert::auth::AuthMatcher;

// Assert no auth was required
AuthMatcher::new(&env).assert_no_auth_required();

// Assert specific auth count
AuthMatcher::new(&env).assert_auth_count(2);
```

## Failure messages

Assertions fail with the counts they observed, which is what makes them worth using over hand-rolled loops:

```text
assertion `left == right` failed: Expected 3 events, found 1
```

<div class="tk-grid tk-grid--2">
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Next step</span>
    <h3 class="tk-card__title">Filtering by contract</h3>
    <p class="tk-card__body"><code>from_contract</code> narrows a matcher to one contract id — essential once a test wires several contracts together.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Testing patterns</h3>
    <p class="tk-card__body">See <a href="../guides/testing-patterns.html">Testing Patterns</a> for event and auth assertions in multi-contract suites.</p>
  </div>
</div>
