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

- Documentation: <https://stellar-crucible.github.io/soroban-testkit/crates/fixtures.html>
- Source: [crates/soroban-testkit-fixtures](https://github.com/stellar-crucible/soroban-testkit/tree/main/crates/soroban-testkit-fixtures)
- Licence: Apache-2.0
