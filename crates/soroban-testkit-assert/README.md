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
```

Matchers read the most recent contract invocation, which is what Soroban SDK v28 exposes through `env.events().all()`.

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/assert.html>
- Source: [crates/soroban-testkit-assert](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-assert)
- Licence: Apache-2.0
