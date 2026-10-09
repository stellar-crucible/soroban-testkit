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
| `EventMatcher::assert_not_emitted()` | No matching event exists |
| `EventMatcher::assert_none_match(pred)` | No matching event satisfies a predicate |
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

## Negative assertions

Proving something did *not* happen is the other half of event testing — a rejected transfer should emit no `Transfer` event, a closed offer should publish nothing at all:

```rust
use testkit_assert::events::EventMatcher;

// Nothing in scope was published
EventMatcher::new(&env).assert_not_emitted();

// A specific topic stayed silent
EventMatcher::new(&env)
    .from_contract(&contract_id)
    .with_topic("Transfer")
    .assert_not_emitted();

// Anything the topic filter cannot express
EventMatcher::new(&env).assert_none_match(|event| {
    matches!(event.type_, soroban_sdk::xdr::ContractEventType::Diagnostic)
});
```

`assert_none_match` hands you the raw `ContractEvent`, so a predicate can inspect the event type or the data payload rather than only the topics.

Both failures quote the event they were not supposed to find:

```text
panicked at 'Expected no events to be emitted, found 1 — unexpected event: topics [Transfer], type Contract'
```

<div class="tk-callout tk-callout--warn">
  <span class="tk-callout__title">"Not emitted" means "not in this invocation"</span>
  <p>The same v28 scope applies to the negative assertions: <code>assert_not_emitted()</code> proves the <strong>most recent invocation</strong> published nothing matching, not that no earlier call did. Run the assertion immediately after the call under test. Aggregating across a whole test is <a href="https://github.com/stellar-crucible/soroban-testkit/issues/19">issue #19</a>.</p>
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
