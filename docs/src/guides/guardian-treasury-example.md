# Guardian-Protected Multisig Treasury

`examples/guardian_treasury` is a `#![no_std]` treasury demonstrating M-of-N
withdrawal approvals with a separate Guardian veto role. The proposal's
timelock begins when the approval threshold is reached. Only the configured
Guardian may veto, and only before the deadline; once vetoed, the proposal
cannot be executed. A signer still needs to satisfy the threshold and wait for
the delay before execution.

The example tracks treasury funds as an internal balance so the authorization,
proposal, veto and accounting workflow can be tested independently. It does
not implement a production token custody or recovery mechanism.

## Workflow

```rust
let treasury_id = env.register(GuardianTreasury, ());
let treasury = GuardianTreasuryClient::new(&env, &treasury_id);

treasury.initialize(&admin, &signers, &2, &3_600, &guardian);
treasury.deposit(&depositor, &10_000);

let id = treasury.propose(&signer_a, &recipient, &2_500);
treasury.approve(&signer_a, &id);
treasury.approve(&signer_b, &id); // starts the delay window
treasury.veto(&guardian, &id);    // optional, only before ready_at
```

If the Guardian does not veto, a signer can execute after the deadline. The
contract emits typed events for initialization, deposits, proposals, approvals,
vetoes and execution. All proposals are persistent entries and the active
configuration and balance are instance data.

## Test layout

The four dedicated suites provide over 30 tests:

- `fixtures.rs`: initialization validation, deposits, proposals and IDs.
- `assert.rs`: guardian authority, veto timing, threshold enforcement,
  timelock execution, events and auth assertions.
- `budget_storage.rs`: instruction baselines, storage tiers and budget checks.
- `proptest.rs`: properties proving vetoed proposals never execute and never
  debit funds, and that execution respects threshold and time.

The example uses Testkit fixtures for isolated environments, Testkit assert
helpers for contract/auth/event assertions, Testkit core for instruction and
storage accounting, and Testkit generators for randomized amounts.

## Budget baselines

`examples/guardian_treasury/budget.json` records guardian-veto and execution
costs. Verify them with:

```bash
cargo test -p guardian-treasury --test budget_storage
```

Regenerate the measurements deliberately with:

```bash
cargo test -p guardian-treasury --test budget_storage -- --ignored record_budget_baseline
```
