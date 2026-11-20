# Flash Mint & Token Burn Worked Example

This guide walks through `examples/flash_mint`, an ERC-3156 / SEP-style Flash Mint token contract that demonstrates cross-contract callback handling, atomic token burning, and comprehensive testing across all four crates in the Testkit workspace.

## What Flash Minting Does

Flash minting allows a borrower contract to temporarily mint unbacked tokens within a single transaction, under the strict mathematical condition that the exact borrowed principal plus an optional protocol fee is returned and burned before invocation completion.

If the borrower fails to execute the callback successfully, returns the wrong magic symbol, or lacks sufficient funds to cover the principal and fee, the entire transaction rolls back atomically.

```rust
use soroban_testkit_example_flash_mint::{FlashMintToken, FlashMintTokenClient};

let token_id = env.register(FlashMintToken, ());
let client = FlashMintTokenClient::new(&env, &token_id);

client.initialize(&admin, &treasury, &10, &1_000_000); // 10 bps fee, 1M max borrow

// Executes callback on receiver, verifies repayment, burns principal, and collects fee
client.flash_loan(&receiver_contract_id, &token_id, &250_000, &Bytes::new(&env));
```

## Protocol Architecture

The protocol adheres to standard flash lending conventions:

1. **Borrow Limit Check**: Asserts `0 < amount <= max_flash_loan` and checks that the contract is not paused.
2. **Reentrancy Protection**: Employs a reentrancy lock in temporary storage to prevent recursive reentrant calls during the borrower callback.
3. **Optimistic Minting**: Mints `amount` tokens to `receiver`.
4. **Cross-Contract Callback**: Calls `receiver.on_flash_loan(initiator, token, amount, fee, data)`. The borrower must return `symbol_short!("success")`.
5. **Atomic Repayment & Burn**: The contract verifies that `receiver` has at least `amount + fee` tokens, burns the borrowed `amount`, and transfers `fee` to the treasury.
6. **Supply Invariance**: Asserts that `total_supply` after repayment is identical to initial supply before the flash mint.

### Error Conditions

| Error | Cause |
|-------|-------|
| `Paused` | Contract is paused by admin |
| `ZeroAmount` | Requested borrow amount is zero or negative |
| `ExceedsMaxFlashLoan` | Amount exceeds configured ceiling |
| `InvalidToken` | Token parameter does not match the Flash Mint contract address |
| `CallbackFailed` | Borrower callback reverted or returned unexpected symbol |
| `DeficitAfterFlashMint` | Borrower spent the borrowed funds without returning them |
| `ReentrancyGuard` | Recursive flash loan invocation attempted during callback |

## Test Suite Structure

The test suite in `examples/flash_mint/tests` is organized into 4 dedicated test files exercising all 4 Testkit crates:

```text
examples/flash_mint/tests/
├── fixtures.rs                 # soroban-testkit-fixtures: TestContext, limits, fees, congestion
├── assert_events_and_auth.rs   # soroban-testkit-assert: EventMatcher, EventLog, failure rollbacks
├── budget_and_storage.rs       # soroban-testkit-core: BudgetGuard, StorageSnapshot, quotas & fragmentation
└── property.rs                 # proptest & generators: Total supply invariants and fee scaling
```

### 1. Fixtures (`tests/fixtures.rs`)

Exercises `soroban-testkit-fixtures`:
- Sets up test environments with `TestContext` and `TestContextBuilder`.
- Configures fee basis points and verifies fee calculations across various amounts.
- Tests borrow limit enforcement and pause/unpause toggles.
- Emulates network queue latency using `CongestionSimulator`.

### 2. Event and Auth Assertions (`tests/assert_events_and_auth.rs`)

Exercises `soroban-testkit-assert`:
- `EventMatcher` asserts emission of `flash_mnt`, `mint`, `burn`, and `fee_paid` events.
- `EventLog` tracks event sequences across multiple sequential flash loans.
- Tests transaction rollback on `CallbackFailed` and `DeficitAfterFlashMint`.
- Validates reentrancy protection against recursive borrowers.

### 3. Budget and Storage Verification (`tests/budget_and_storage.rs`)

Exercises `soroban-testkit-core`:
- `BudgetGuard` and `BudgetSnapshot` measure CPU instruction and memory consumption.
- `StorageSnapshot` validates atomic storage invariance before and after a flash loan.
- `assert_storage_tier_count!` verifies entry quotas across instance, persistent, and temporary tiers.
- `StorageFragmentationAnalyzer` inspects storage layout for serialization efficiency.

### 4. Property Testing (`tests/property.rs`)

Uses `proptest` to test protocol invariants:
- **Total Supply Invariant**: Verifies that for any arbitrary borrow amount, `total_supply` before and after the flash mint is strictly identical.
- **Fee Scaling Invariant**: Verifies that fee computation matches mathematical expectations across all inputs.
- **Bound Enforcing**: Proves that requested amounts exceeding `max_flash_loan` are consistently rejected.
