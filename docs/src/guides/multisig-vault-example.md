# Multisig Vault Worked Example

This guide walks through `examples/multisig_vault`, a time-delayed M-of-N
treasury vault that demonstrates every crate in the Testkit workspace.

## What the Vault Does

The vault holds a balance and requires two independent conditions before any
withdrawal can be executed:

1. **Threshold** — at least `threshold` of the configured `signers` must approve
   the proposal.
2. **Timelock** — once the threshold is reached the proposal is queued, and it
   cannot execute until `timelock` seconds have elapsed. During the delay any
   signer can cancel the request.

```rust
use multisig_vault::{MultisigVault, MultisigVaultClient};

let vault_id = env.register(MultisigVault, ());
let client = MultisigVaultClient::new(&env, &vault_id);

client.initialize(&admin, &signers, &2, &3_600);
client.deposit(&depositor, &10_000);

let proposal_id = client.propose_withdrawal(&signer_a, &recipient, &2_500);
client.approve(&signer_a, &proposal_id);
client.approve(&signer_b, &proposal_id); // threshold reached, proposal queued

env.ledger().set_timestamp(env.ledger().timestamp() + 3_600);
client.execute(&signer_a, &proposal_id);
```

## Contract Surface

| Function | Purpose |
|----------|---------|
| `initialize` | Stores signers, threshold, timelock and starts the balance at zero |
| `deposit` | Requires the depositor's auth and credits the vault |
| `propose_withdrawal` | Signer-only; validates amount against the balance |
| `approve` | Records a unique signer approval; queues the proposal at threshold |
| `execute` | Requires threshold + timelock + sufficient balance |
| `cancel` | Any signer may veto a proposal until it executes |
| `balance`, `threshold`, `timelock`, `signers`, `proposal`, `next_proposal_id` | Read-only getters |

Failures are returned as typed `VaultError` values, so tests can assert exact
error causes rather than matching panic strings. State changes emit typed
contract events (`WithdrawalProposed`, `WithdrawalApproved`,
`WithdrawalExecuted`, `WithdrawalCancelled`).

Storage uses the typed `DataKey` enum (`DataKey::Signers`,
`DataKey::Proposal(u64)`, and so on) so that the vault's keys are namespaced and
auditable.

## Test Layout

The suite is split across four files under `examples/multisig_vault/tests` and
contains more than 30 tests:

| File | Focus |
|------|-------|
| `fixtures.rs` | Initialization, deposits, proposal creation, approvals and validation errors |
| `assert.rs` | Event and authorization assertions, timelock rejection, execution and cancellation |
| `budget_storage.rs` | Budget snapshots, network profiles, namespace auditing and fee estimates |
| `proptest.rs` | Property tests proving funds cannot leave without threshold + delay |

A shared `tests/common/mod.rs` builds a `Fixture` that registers the contract,
generates signers and exposes helpers for funding, proposing, approving and
advancing ledger time.

### Fixtures and Setup

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;
use soroban_testkit_fixtures::NetworkProfile;

let ctx = TestContextBuilder::new()
    .with_network_profile(NetworkProfile::Local)
    .with_users(2)
    .build();
```

`Local` keeps the host unmetered for fast iteration. Swap in
`NetworkProfile::Testnet` or `NetworkProfile::Mainnet` to enforce real resource
ceilings and ledger parameters without changing the test body.

### Authorization and Event Assertions

```rust
use soroban_testkit_assert::auth::AuthMatcher;
use soroban_testkit_assert::events::EventMatcher;

fixture.fund(1_000);
EventMatcher::new(&fixture.ctx.env).assert_count(1);
AuthMatcher::new(&fixture.ctx.env).assert_auth_count(1);
```

### Budget and Storage Inspection

```rust
use soroban_testkit_core::budget::BudgetSnapshot;
use soroban_testkit_core::namespace_auditor::NamespaceAuditor;

let before = BudgetSnapshot::capture(&ctx.env.host().budget_cloned());
client.execute(&signer, &proposal_id);
let after = BudgetSnapshot::capture(&ctx.env.host().budget_cloned());
let cost = before.diff(&after);

let mut auditor = NamespaceAuditor::default();
auditor.record("multisig_vault", "DataKey::Signers");
assert!(!auditor.audit().has_collisions());
```

### Property Tests

```rust
proptest! {
    #[test]
    fn funds_never_leave_without_threshold(amount in withdrawal_amount()) {
        let fixture = Fixture::funded();
        let proposal_id = fixture.propose(amount);
        fixture.advance_time(TIMELOCK);

        let result = fixture.client().try_execute(&fixture.signers[0], &proposal_id);
        prop_assert_eq!(result.unwrap_err().unwrap(), VaultError::ThresholdNotMet);
        prop_assert_eq!(fixture.client().balance(), INITIAL_BALANCE);
    }
}
```

The property suite also covers premature execution, exact balance accounting,
cancelled proposals, duplicate approvals, `ready_at` computation, and monotonic
proposal IDs. Amounts are produced with
`soroban_testkit_generators::strategies::token_amount()`.

## Running the Example

From the repository root:

```bash
cargo test -p multisig-vault
cargo clippy -p multisig-vault --all-targets -- -D warnings
```

Because `examples/multisig_vault` is a workspace member, the top-level commands
exercise it too:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets --locked -- -D warnings
```
