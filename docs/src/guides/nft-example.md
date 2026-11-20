# Worked Example: Non-Fungible Token (NFT) with Royalties and Approvals

A standard-compliant Non-Fungible Token (NFT) contract on Soroban: unique token IDs, single and batch minting, operator approvals with expiration, safe transfers, metadata URIs, and secondary sale royalty splits. Everything here is real code in [`examples/nft`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/nft), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/nft</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">27 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-nft</span></div>
</div>

## The contract

One [`#![no_std]`](https://doc.rust-lang.org/reference/attributes.html) contract implements token ownership, operator authorizations, and marketplace royalty settlements:

```text
Mint Token ──▶ Set Approvals (Single/All) ──▶ Transfer / Safe Transfer ──▶ Secondary Sale with Royalty Split
```

### Unique token IDs and owner tracking

Each token has a unique `u32` ID, an owner address, and a metadata URI:
- Minting verifies the token ID does not already exist, records the owner, assigns the URI, and increments supply and balances.
- Burning cleans up ownership and URI mappings and decrements total supply.

### Approvals and operator delegation

Two levels of authorization allow contracts and marketplaces to operate on behalf of users:
1. **Single Token Approval**: An owner approves an operator for a specific `token_id` valid until a ledger sequence (`live_until_ledger`).
2. **Operator Approval for All**: An owner authorizes an operator to manage all tokens they own until expiration.
3. **Revocation & Expiration**: Approvals can be explicitly revoked or expire automatically as the ledger sequence advances.

### Secondary sale royalty splits

When an NFT is sold on a secondary market:
- The contract computes the creator royalty based on `royalty_bps` (e.g., 500 bps = 5%).
- Royalty proceeds are delivered directly to `royalty_recipient`.
- Remaining proceeds are transferred to the seller.
- Token ownership transfers to the buyer in the same atomic invocation.

## Why four files

The test suite divides its coverage cleanly across four dedicated files, exercising all four workspace testkit crates:

| File | Crate | What it covers |
| --- | --- | --- |
| `tests/fixtures.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, user roles, minting, batch minting, transfers, approval expiration, burning, and secondary sales |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `assert_auth_matches!`, `assert_event_topic_count!`, and events for mint, transfer, approval, and royalties |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | Storage inspection verifying persistent entries per token ID, and storage growth benchmarking across 1, 10, and 100 minted tokens |
| `tests/property.rs` | `soroban-testkit-generators` | Proptest suite proving token uniqueness, single-owner invariant, batch balance conservation, and bounded royalty math |

## Key test patterns

### Single-owner invariant property test

`tests/property.rs` asserts that each token ID has exactly one owner at any time, that duplicates are rejected, and that transfers preserve total supply and balance invariants:

```rust
#[test]
fn proptest_token_uniqueness_and_single_owner_invariant(token_id in 1u32..=1000u32) {
    // Mint token to user_a
    client.mint(&user_a, &token_id, &uri);
    prop_assert_eq!(client.owner_of(&token_id), Some(user_a.clone()));

    // Uniqueness invariant: duplicate mint is rejected
    let duplicate = client.try_mint(&user_b, &token_id, &uri);
    prop_assert!(duplicate.is_err());

    // Transfer preserves single-owner invariant
    client.transfer(&user_a, &user_b, &token_id);
    prop_assert_eq!(client.owner_of(&token_id), Some(user_b.clone()));
    prop_assert_eq!(client.balance_of(&user_a), 0);
    prop_assert_eq!(client.balance_of(&user_b), 1);
}
```

### Storage growth benchmarking

`tests/budget_and_storage.rs` measures persistent storage expansion across 1, 10, and 100 minted tokens using `StorageSnapshot`:

```rust
assert_eq!(storage_1.len(), 4);   // Config + Balance + Owner + URI
assert_eq!(storage_10.len(), 22);  // Config + Balance + 10 Owners + 10 URIs
assert_eq!(storage_100.len(), 202);// Config + Balance + 100 Owners + 100 URIs
```
