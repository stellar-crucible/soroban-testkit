# Troubleshooting

Common issues encountered when testing Soroban smart contracts, how to identify them, and how to resolve them.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Section</span><span class="tk-spec__value">Getting Started</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Relevant Crates</span><span class="tk-spec__value">fixtures · assert · core</span></div>
</div>

---

## 1. Duplicate `soroban-sdk` versions in dependency tree

### Symptom

The compiler produces a mismatched types error between two identical-looking types:

```text
error[E0308]: mismatched types
  --> tests/contract.rs:12:35
   |
12 |     let client = ContractClient::new(&env, &contract_id);
   |                  ------------------- ^^^^ expected `&soroban_sdk::Env`, found `&soroban_sdk::Env`
   |
   = note: `soroban_sdk::Env` and `soroban_sdk::Env` have similar names, but are considered distinct types
   = note: perhaps two different versions of a crate are being used?
```

### Cause

Cargo resolved two distinct versions of `soroban-sdk` in your dependency graph. Because Rust treats types from different crate versions as incompatible, an `Env` from one version cannot be passed to a contract client built against another.

Run `cargo tree -d soroban-sdk` to inspect duplicates:

```text
soroban-sdk v28.0.0
└── my-tests v0.1.0
soroban-sdk v22.0.0
└── other-dependency v0.2.0
```

### Fix

Align all crates and workspace members to the same `soroban-sdk` version:

```toml
[dependencies]
soroban-sdk = "28.0.0"
```

Then update your lockfile:

```bash
cargo update -p soroban-sdk
```

---

## 2. `testutils` feature not enabled on `soroban-sdk`

### Symptom

Calling `Address::generate` or budget inspection methods fails to compile:

```text
error[E0599]: no function or associated item named `generate` found for struct `soroban_sdk::Address` in the current scope
  --> tests/test.rs:8:26
   |
8  |     let addr = Address::generate(&env);
   |                         ^^^^^^^^ function or associated item not found in `Address`
```

### Cause

Test helpers like `Address::generate(&env)` and ledger controls are guarded behind the SDK's `testutils` feature flag.

### Fix

Enable the `testutils` feature in your `Cargo.toml` under `[dev-dependencies]`:

```toml
[dev-dependencies]
soroban-sdk = { version = "28.0.0", features = ["testutils"] }
```

And bring the test trait into scope in your test file:

```rust
use soroban_sdk::testutils::Address as _;
```

Alternatively, use [`TestContextBuilder`](../crates/fixtures.md) from `soroban-testkit-fixtures`, which pre-generates addresses without manual setup.

---

## 3. `Address` used across different `Env` instances

### Symptom

A test panics with a context error:

```text
thread 'test' panicked at 'Context error: BadContext'
```

### Cause

In the Soroban SDK, an `Address` instance is a host handle bound to the specific `Env` that created it. Passing an `Address` created in one `Env` into a contract invocation on a second `Env` produces a context mismatch.

### Fix

When resetting or creating a fresh environment, migrate addresses across environment boundaries using [`TestContext::reset()`](../crates/fixtures.md):

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;

let mut ctx = TestContextBuilder::new().with_users(2).build();
// ... test phase 1 ...

// Safely resets the chain while preserving addresses in the new env
ctx.reset();

// ... test phase 2 with same ctx.admin and ctx.user(0) ...
```

---

## 4. Events missing from previous contract invocations

### Symptom

An event that was emitted earlier in a test is not found by [`EventMatcher`](../crates/assert.md):

```text
thread 'test' panicked at 'Expected an event whose data matches, but no event was emitted'
```

### Cause

By design, `EventMatcher::new(&env)` scopes event inspection to the **most recent contract invocation**. If your test calls multiple contract methods sequentially, matching against `EventMatcher::new(&env)` only inspects the last call.

### Fix

To aggregate and assert events across multiple calls, use [`EventLog`](../crates/assert.md):

```rust
use soroban_testkit_assert::events::EventLog;

let mut log = EventLog::new();

// Collect events from each call
log.collect(&env, || client.mint(&user, &100));
log.collect(&env, || client.transfer(&user, &recipient, &50));

// Assert over the full history
log.matcher(&env).with_topic("mint").assert_emitted();
log.matcher(&env).with_topic("transfer").assert_emitted();
```

<div class="tk-callout tk-callout--tip">
  <span class="tk-callout__title">Quick check</span>
  <p>For single invocations, <code>EventMatcher::new(&env)</code> is all you need. For sequences where early events matter, wrap the sequence in <code>EventLog</code>.</p>
</div>
