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
```

Matchers read the most recent contract invocation, which is what Soroban SDK v28 exposes through `env.events().all()`.

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/assert.html>
- Source: [crates/soroban-testkit-assert](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-assert)
- Licence: Apache-2.0
