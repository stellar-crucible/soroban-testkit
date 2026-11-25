# Worked Example: Decentralized Escrow with Arbiter Dispute Resolution

A milestone-based escrow contract with third-party arbiter dispute resolution: a buyer funds the escrow for a seller across multiple milestones, the buyer (or designated arbiter) approves milestone releases upon satisfactory delivery, either party can raise a formal dispute to freeze payouts, and the arbiter can resolve the dispute with arbitrary split payouts or the buyer can claim a timeout refund.

Everything here is real code in [`examples/escrow`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/escrow), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/escrow</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">35+ passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-escrow</span></div>
</div>

## Architecture and State Machine

The contract is `#![no_std]` and manages an escrow agreement between three parties: Buyer, Seller, and Arbiter.

```text
       ┌───────────┐
       │   Active  │◀──────────────────┐
       └─────┬─────┘                   │
             │                         │
      ┌──────┴──────┐                  │
      ▼             ▼                  │
┌───────────┐ ┌───────────┐            │
│ Completed │ │  Disputed ├────────────┘
└───────────┘ └─────┬─────┘ (milestone approval)
                    │
            ┌───────┴───────┐
            ▼               ▼
      ┌───────────┐   ┌───────────┐
      │  Resolved │   │  Refunded │
      └───────────┘   └───────────┘
```

### Storage Model

- **Instance Storage**:
  - `DataKey::Config`: The immutable parties, total allocation, deposited amount, released amount, timeout ledger, and status.
  - `DataKey::MilestoneIds`: Vector of registered milestone IDs.
- **Persistent Storage**:
  - `DataKey::Milestone(u32)`: Independent milestone record with description, amount, and `MilestoneStatus` (`Pending`, `Approved`, `Released`).

---

## Testing Across All Four Testkit Crates

The test suite in `examples/escrow` demonstrates how each Testkit crate provides dedicated superpowers:

### 1. `soroban-testkit-fixtures` (`tests/fixtures_tests.rs`)
Manages multi-identity fixtures (`buyer`, `seller`, `arbiter`, `attacker`), deploys mock Stellar asset tokens, and controls ledger progression:

```rust
let mut fix = EscrowFixture::new(10_000, 500);
fix.client.deposit(&fix.buyer, &10_000);

// Advance mock ledger past expiration sequence
fix.ctx.advance_ledger(501);

let refunded = fix.client.claim_refund(&fix.buyer);
assert_eq!(refunded, 10_000);
assert_eq!(fix.client.get_status(), EscrowStatus::Refunded);
```

### 2. `soroban-testkit-assert` (`tests/assert_tests.rs`)
Asserts emitted contract events and verifies strict mathematical token conservation across all parties:

```rust
// Initial total tokens must strictly equal final total tokens
assert_eq!(
    initial_buyer + initial_seller,
    final_buyer + final_seller + contract_balance
);
assert_eq!(contract_balance, 0);
```

### 3. `soroban-testkit-core` (`tests/budget_and_storage.rs`)
Meters CPU instructions, memory bytes, and verifies storage tier isolation (Instance vs Persistent):

```rust
client.initialize(&buyer, &seller, &arbiter, &token, &10_000, &500);
client.add_milestone(&buyer, &1, &Symbol::new(&env, "m1"), &5_000);

let budget = BudgetSnapshot::last_invocation(&env);
assert!(budget.cpu_insns > 0);

let storage = StorageSnapshot::capture(&env, &contract);
let persistent_entries = storage.entries_in(StorageTier::Persistent);
assert_eq!(persistent_entries.len(), 1);
```

### 4. `soroban-testkit-generators` (`tests/property_tests.rs`)
Property-based invariants running hundreds of generated scenarios with `proptest!`:

```rust
proptest! {
    #[test]
    fn prop_unauthorized_parties_cannot_claim_or_release(
        total in 1_000i128..=100_000,
        milestone_amt in 100i128..=500,
    ) {
        // Invariant: Attackers can never approve milestones, resolve disputes, or claim refunds
        let res_approve = client.try_approve_milestone(&attacker, &1);
        prop_assert_eq!(res_approve, Err(Ok(EscrowError::Unauthorized)));
    }
}
```

---

## Running the Suite

```bash
cargo test -p soroban-testkit-example-escrow
```
