# Worked Example: Flash Loan Provider and Receiver

The [`examples/flash_loan`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/flash_loan)
crate is a flash loan market in two `#![no_std]` contracts:

- **`FlashLoanProvider`** holds liquidity in any number of Stellar Asset
  Contract tokens. It lends to a receiver contract, calls it back, and then
  checks its own balance. If the balance has not grown by at least the fee, it
  returns an error and the host reverts the whole invocation, including the
  principal transfer. Fees are attributed per token and per borrower.
- **`FlashLoanReceiver`** is a configurable borrower. Its `RepayMode` (`Full`,
  `ShortBy(n)`, `PrincipalOnly`, `Reject`) drives every failure path the tests
  need.

The suite spreads 33 tests over four files, one per testkit crate.

## The loan, step by step

```text
borrower ──flash_loan──▶ provider ──transfer(amount)──▶ receiver
                            │                              │
                            │◀── on_flash_loan(fee) ───────┤
                            │◀── transfer(amount + fee) ───┘
                            └─ balance ≥ before + fee ? record fee : Err(RepaymentShort)
```

- `compute_fee(amount, bps)` rounds up, so a positive loan is free only when the
  rate is zero. The rate is capped at `MAX_FEE_BPS` (10%).
- `flash_loan_multi` lends several tokens at once, with one callback and a
  repayment check per token. Duplicate tokens and mismatched lists are refused.
- No reentrancy lock is needed, because the Soroban host never lets a contract
  re-enter itself.

## `tests/fixtures.rs`: soroban-testkit-fixtures

The market is built on a `TestContext`, and `TestContextBuilder::with_users(2)`
supplies two borrowers. That makes it possible to check that fees land on the
right borrower and token:

```rust,ignore
let ctx = TestContextBuilder::new().with_users(2).build();
let market = Market::on(ctx.env.clone());
let a1 = lender.flash_loan(ctx.user(0), &market.receiver, &market.tokens[0], &100_000, &data);
let b1 = lender.flash_loan(ctx.user(1), &market.receiver, &market.tokens[0], &300_000, &data);
assert_eq!(lender.fees_collected(&market.tokens[0]), a1 + b1);
```

This file also covers fee rounding (`flash_fee(10_001) == 10` at 9 bps), deposits,
multi-token fees, invalid amounts, unlisted tokens, insufficient liquidity, and
admin configuration.

## `tests/assert.rs`: soroban-testkit-assert

**Rollback.** Repaying one stroop short, returning only the principal,
refusing the callback, or coming up short on one leg of a multi-token loan all
fail with an exact error, and every balance, fee counter, and loan counter is
back where it started:

```rust,ignore
market.set_mode(RepayMode::ShortBy(1));
assert_contract_error!(try_borrow(&market, 1_000_000), FlashLoanError::RepaymentShort);
assert_rolled_back(&market);
```

**Propagated errors.** A receiver that does not trust the lender panics with
`ReceiverError::UnknownLender`. The error reaches the test with the
receiver's code, so it is checked with `assert_host_error!`.

**Events.** `EventMatcher` finds the provider's `flash` event, and
`assert_event_topic_count!` counts the two token transfers (principal out,
repayment in).

**Sub-invocation authorization.** The borrower signs only the root call:

```rust,ignore
market.lender().mock_auths(&[MockAuth {
    address: &market.borrower,
    invoke: &MockAuthInvoke {
        contract: &market.provider,
        fn_name: "flash_loan",
        args: (borrower, receiver, token, amount, data).into_val(env),
        sub_invokes: &[],
    },
}]).flash_loan(/* ... */);
```

No nested entries are needed:
- the principal leaves the provider's own balance
- the receiver authenticates the provider as its direct caller (`provider.require_auth()`)
- the repayment leaves the receiver's own balance

Without the borrower's signature, the loan is refused.

## `tests/budget_and_storage.rs`: soroban-testkit-core

Single-token and two-token loans are measured with
`BudgetSnapshot::last_invocation`. The measurements are compared against the
committed [`examples/flash_loan/budget.json`](https://github.com/stellar-crucible/soroban-testkit/blob/main/examples/flash_loan/budget.json)
with a 10% tolerance, and the single loan also runs under a `BudgetGuard`
ceiling. After an SDK bump, re-record the baseline with:

```bash
cargo test -p soroban-testkit-example-flash-loan -- --ignored budget_baseline_records_current_costs
```

`StorageSnapshot` diffs show the storage footprint:
- a first loan adds exactly two persistent entries (`FeesCollected`,
  `BorrowerFees`) and one instance entry (`LoanCount`)
- a returning borrower rewrites those entries in place
- a failed loan leaves provider storage untouched

`assert_storage_tier_count!` keeps the provider free of temporary state.

## `tests/property.rs`: soroban-testkit-generators

`token_amount()` and `basis_points()` drive proptest over the fee function:

- the fee is the exact ceiling of `amount × bps / 10_000`
- it is monotonic in the amount, never exceeds the principal, and is zero only
  at a zero rate
- negative amounts and overflowing products return `None` instead of wrapping

End to end, every affordable loan grows the pool by exactly its fee, any
shortfall rolls back, and per-borrower fees always add up to the token total.

## Running it

```bash
cargo test -p soroban-testkit-example-flash-loan
cargo clippy --workspace --all-targets --locked -- -D warnings
```
