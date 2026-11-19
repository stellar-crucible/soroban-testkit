# soroban-testkit-assert

Fluent matchers over the events and authorisations of a Soroban contract call, so assertions read like the behaviour you expect.

```rust
use soroban_testkit_assert::events::EventMatcher;

EventMatcher::new(&env)
    .from_contract(&contract_id)
    .with_topic("Transfer")
    .assert_emitted();

EventMatcher::new(&env)
    .with_topic("Approval")
    .assert_not_emitted();

// and what an event carried, typed or field by field
EventMatcher::new(&env)
    .with_topic("Transfer")
    .assert_data_matches(|data| data.deserialize::<i128>() == Some(1_000));

EventMatcher::new(&env)
    .with_topic("Incremented")
    .assert_data_matches(|data| {
        data.field("new_count")
            .and_then(|value| value.deserialize::<u32>())
            == Some(12)
    });

// Validate the full shape of a contractevent payload:
soroban_testkit_assert::assert_event_data_schema!(&env, "transfer", {
    amount: i128,
    from: soroban_sdk::Address,
    to: soroban_sdk::Address,
});

// Decode failed try_* calls instead of using an opaque unwrap():
let value = soroban_testkit_assert::assert_no_contract_error!(client.try_get());
```

A matcher reads the invocation that just ran — what Soroban SDK v28 exposes through `env.events().all()`. Gather several calls to assert over them as one set:

```rust
use soroban_testkit_assert::events::EventLog;

let mut log = EventLog::new(&env);
client.transfer(&from, &to, &100);
log.collect();
client.close_offer(&from);
log.collect();

log.matcher().with_topic("Transfer").assert_count(1);
log.matcher().with_topic("Refund").assert_not_emitted();
```

### Relative Gas Savings Comparison

Compare baseline and optimized algorithm implementations and assert relative gas savings:

```rust
use soroban_testkit_assert::assert_budget_diff_within;

assert_budget_diff_within!(
    &env,
    baseline: { client.linear_search(&items, &target) },
    optimized: { client.binary_search(&items, &target) },
    min_cpu_savings_percent: 20.0,
);
```

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/assert.html>
- Source: [crates/soroban-testkit-assert](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-assert)
- Licence: Apache-2.0
