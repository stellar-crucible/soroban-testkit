# soroban-testkit-fixtures

One call to get a mocked `Env`, an admin address and pre-generated users, so a test starts at the behaviour instead of at the boilerplate.

```rust
use soroban_testkit_fixtures::{TestContext, TestContextBuilder};

let ctx = TestContext::new();
let ctx = TestContextBuilder::new()
    .with_users(3)
    .without_mock_auths()
    .build();

let (env, admin, users) = (&ctx.env, &ctx.admin, &ctx.users);
```

Move the ledger for time-dependent contracts:

```rust
let mut ctx = TestContext::new();
ctx.set_timestamp(1_700_000_000);
ctx.advance_time(86_400 * 30); // 30 days later, sequence untouched
ctx.advance_ledger(10);        // 10 closes later, clock untouched
```

Clear the chain between phases without losing your actors:

```rust
ctx.reset();      // fresh env: ledger, events, auths and storage gone, same addresses
ctx.reset_full(); // ... and brand-new addresses too
```

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/fixtures.html>
- Source: [crates/soroban-testkit-fixtures](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-fixtures)
- Licence: Apache-2.0
