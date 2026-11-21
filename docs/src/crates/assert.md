# `soroban-testkit-assert`

`soroban-testkit-assert` provides assertions for Soroban contracts, including event counts, state conservation, storage quotas, and least-privilege authorization checks.

## `assert_auth_invoker_distinct!`

To enforce the principle of least privilege, contracts must ensure that privileged admin operations cannot be authorized by unauthorized user addresses or unintended contract callers.

The `assert_auth_invoker_distinct!` macro verifies that a given invocation was *only* authorized by the declared authorized address and fails with a security diagnostic if any other address provided authorization.

```rust
use soroban_testkit_assert::assert_auth_invoker_distinct;

// Asserts that only admin_address signed/authorized the operation
assert_auth_invoker_distinct!(&admin_address, &[admin_address.clone()]);
```
