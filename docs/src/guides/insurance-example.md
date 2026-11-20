# Decentralized Insurance & Mutual Claim Payout Worked Example

This guide walks through `examples/insurance`, a mutual insurance pool. Capital providers underwrite the pool by depositing an asset and receiving shares of its reserves. Policyholders buy coverage for a period and pay a premium priced off an annual rate. When a policyholder files a loss, a panel of assessors votes; a claim that reaches quorum with a majority of approvals is settled from the reserves, and the loss is shared by every provider through the share price rather than by whichever provider withdraws first.

The suite has 50 tests across 4 files and uses all four workspace crates.

## The contract

One contract does the whole job, so the insurance and underwriting scenarios can be read without jumping between modules:

| Item | Role |
|------|------|
| `InsurancePool` | Holds capital and premiums, issues shares, records policies and claims, tallies assessor votes, and pays approved losses |
| Capital token | A Stellar Asset Contract standing in for the asset providers deposit and claims are paid in |

`InsurancePool` is `#![no_std]`. Amounts are in the capital token's smallest unit and every ratio is in basis points.

```rust
use soroban_testkit_example_insurance::*;

let token = env.register_stellar_asset_contract_v2(admin.clone()).address();
let pool_id = env.register(InsurancePool, ());
let pool = InsurancePoolClient::new(&env, &pool_id);

pool.initialize(&InsuranceConfig {
    admin: admin.clone(),
    capital_token: token.clone(),
    assessors: vec![&env, assessor_a, assessor_b, assessor_c],
    quorum: 2,                  // two of three votes resolve a claim
    premium_rate_bps: 500,      // 5% a year on coverage
    max_coverage: 1_000_000 * ONE,
});
```

## Protocol mechanics

### Underwriting

`deposit(provider, amount)` moves capital into the pool and mints shares priced against the reserves already there:

```text
shares = amount × total_shares / reserves        (one-for-one while the pool is empty)
```

The first deposit — and the first deposit into a pool whose reserves were wiped out — mints one share per unit. Every later deposit buys shares at the current price, so a provider arriving after a payout has been absorbed pays the post-loss price instead of buying the old value cheaply.

`withdraw(provider, shares)` pays `shares × reserves / total_shares`, rounded down. A withdrawal can only take what the reserves hold at that moment, so a provider cannot drain capital that backs a pending loss. Burning the last share deletes the provider's storage entry.

### Premiums

`buy_policy(holder, coverage, duration)` charges simple interest on the coverage for the period and adds it to the reserves:

```text
premium = ceil(coverage × premium_rate_bps × duration / (10,000 × 31,536,000))
```

At the default 5% rate, covering 10,000 units for a year costs 500 units; half a year costs 250. The premium is ordinary pool income: it raises the share price for providers and is available to pay claims.

### Claims and assessor voting

| Operation | Who signs | What happens |
|-----------|-----------|--------------|
| `file_claim(holder, policy_id, loss)` | holder | Refused once the policy has expired, when the loss exceeds the coverage, and on a second claim against the same policy. Records a `Pending` claim |
| `vote(assessor, claim_id, approve)` | assessor | One vote per assessor per claim. The claim resolves as soon as quorum is reached |
| `settle(claim_id)` | anyone | Pays an approved claim from the reserves |

A claim resolves on a strict majority: more approvals than rejections approves it, and a tie rejects. Rejections are final and pay nothing.

```rust
let policy = pool.buy_policy(&holder, &(10_000 * ONE), &YEAR);
let claim = pool.file_claim(&holder, &policy.id, &(8_000 * ONE));
assert_eq!(claim.status, ClaimStatus::Pending);

pool.vote(&assessor_a, &claim.id, &true);           // still pending: one of two
let resolved = pool.vote(&assessor_b, &claim.id, &true);
assert_eq!(resolved.status, ClaimStatus::Approved); // quorum reached

let settled = pool.settle(&claim.id);
assert_eq!(settled.payout, 8_000 * ONE);
```

### Payouts are capped by reserves

Settlement pays `min(loss, coverage, reserves)`. A pool holding 1,500 units cannot pay an 8,000-unit loss, so it pays the 1,500 it holds and leaves nothing behind:

```rust
pool.deposit(&provider, &(1_000 * ONE));            // capital
pool.buy_policy(&holder, &(10_000 * ONE), &YEAR);   // + 500 of premium
// reserves = 1,500
let settled = pool.settle(&claim.id);
assert_eq!(settled.payout, 1_500 * ONE);
assert_eq!(pool.reserves(), 0);
```

A pool with nothing left refuses to settle rather than recording a zero payout, so an approved claim stays approved if the reserves are replenished later.

**Solvency holds by construction.** A payout can never exceed the reserves captured before it, `reserves` is never negative, and `total_payouts ≤ total_deposited + total_premiums`. The property test in `tests/proptest.rs` runs random claim frequencies and loss sizes against that invariant.

### Error conditions

| Error | Cause |
|-------|-------|
| `AlreadyInitialized` / `NotInitialized` | Initialized twice / used before initialization |
| `InvalidParameter` | Empty panel, zero quorum, quorum above the panel size, rate above 100%, a non-positive coverage cap, or a policy with zero coverage/coverage above the cap/zero duration |
| `InvalidAmount` | Zero or negative deposit, withdrawal, or claim |
| `NotAssessor` | A vote from an address that is not on the panel |
| `AlreadyVoted` | A second vote from the same assessor on the same claim |
| `ClaimNotPending` | Voting on a claim that already resolved |
| `ClaimNotFound` / `PolicyNotFound` | Unknown id, or a claim filed against a policy the caller does not hold |
| `PolicyNotActive` | Claiming against an expired policy |
| `ClaimExceedsCoverage` | A loss larger than the policy covers |
| `ClaimAlreadyFiled` | A second claim against the same policy |
| `NotApproved` | Settling a pending, rejected, or already-settled claim |
| `InsufficientShares` | Withdrawing more shares than the provider holds |
| `InsufficientReserves` | Withdrawing from, or settling against, an empty pool |
| `Overflow` | Arithmetic outside the supported range |

## Test suite structure

```text
examples/insurance/tests/
├── common/mod.rs      # the `Insurance` fixture: deploys the token and pool, funds actors
├── fixtures.rs        # soroban-testkit-fixtures — lifecycle, shares, premiums, claims, voting, payouts (25 tests)
├── assert.rs          # soroban-testkit-assert — events, auth, balance conservation (10 tests)
├── budget_storage.rs  # soroban-testkit-core — per-call budgets, storage tiers and diffs (10 tests)
└── proptest.rs        # soroban-testkit-generators + proptest — solvency and shares (5 properties)
```

### The `Insurance` fixture (`tests/common/mod.rs`)

Built on `TestContextBuilder::new().with_users(8)`: two capital providers, two policyholders, three assessors, and an outsider who is on no panel. `Insurance::new()` deploys the token and pool, initializes it with the default parameters, and funds each actor with 10,000,000 units. Helpers keep the tests readable:

```rust
let insurance = Insurance::new();
insurance.deposit(&insurance.provider, 1_000_000 * ONE);  // underwrite
insurance.buy(&insurance.holder, 10_000 * ONE, YEAR);     // premium = 500
let (policy, claim) = insurance.open_claim(&insurance.holder, 10_000 * ONE, 8_000 * ONE);
insurance.approve(claim.id);                              // two approving votes
insurance.settle(claim.id);                               // payout = 8,000
insurance.advance_time(YEAR + 1);                         // let a policy expire
```

### 1. Fixtures (`tests/fixtures.rs`)

Drives the whole lifecycle and pins exact numbers: a 1,000,000-unit deposit mints 1,000,000 shares, and a provider arriving after 500 units of premium buys shares below one-for-one. A year of 10,000-unit coverage costs 500 units, half a year costs 250, and the premium lands in the reserves. Filing records a pending claim with no votes and writes the claim id back to the policy. A loss above the coverage, a second claim on the same policy, someone else's policy, and an expired policy are each refused with their own error. Two approvals resolve a claim, a tie rejects it, and settlement pays the loss, moves the tokens, and leaves the stored reserves equal to the token balance. The cap test settles an 8,000-unit claim from a 1,500-unit pool and gets 1,500.

### 2. Assertions (`tests/assert.rs`)

Every mutation publishes one pool event and, where capital moves, one `transfer` from the capital token. The tests pin the payloads (`(amount, shares)` for a deposit, `(id, coverage, premium)` for a policy, `(claim_id, payout)` for a settlement) with `EventMatcher`, count events per contract and per topic, and use `EventLog` across a whole underwrite → premium → claim → vote → vote → settle lifecycle:

```rust
assert_event_topic_count!(log, from = insurance.pool_id, "vote", 2);
assert_event_topic_count!(log, from = insurance.token, "transfer", 3);
assert_event_count_range!(log, 9);
```

`assert_auth_matches!` proves that `deposit`, `buy_policy`, `file_claim`, `vote` and `withdraw` each require their own actor with the exact arguments, and `assert_balance_conserved!` shows that a settlement only moves tokens between pool and holder while a deposit mints none.

### 3. Budget & storage (`tests/budget_storage.rs`)

Every user-facing call — deposit, buy policy, file claim, both votes, settle, withdraw — runs under a `BudgetGuard` with a CPU and memory ceiling, so a regression that turns one storage write into many fails the suite instead of quietly raising the fee. Storage is pinned with `StorageSnapshot` diffs and `assert_storage_tier_count!`:

* the first deposit adds exactly one persistent entry, and each provider gets their own;
* a policy and its claim are separate entries, and every vote adds one more;
* withdrawing the last share removes the entry;
* voting and settling rewrite the claim in place;
* `StorageFragmentationAnalyzer` produces a report over the pool's storage.

Because `initialize` writes every counter as zero, the first deposit modifies instance storage instead of growing it — which is why the diff is exactly one entry.

### 4. Properties (`tests/proptest.rs`)

Capital sizes come from `soroban_testkit_generators::strategies::token_amount()`.

| Property | Invariant |
|----------|-----------|
| `prop_payouts_never_exceed_the_reserves` | Across random claim frequencies and loss sizes, each payout is bounded by the reserves captured immediately before settlement, and lifetime payouts stay inside capital plus premiums |
| `prop_shares_track_every_deposit` | Whatever the deposit order and size, issued shares equal the shares providers hold and reserves equal the capital behind them |
| `prop_withdraw_never_takes_more_than_the_reserves` | A withdrawal pays a proportional slice of the reserves and cannot overdraw them |
| `prop_premium_is_simple_interest_on_the_coverage` | The premium matches the rounded simple-interest formula for any coverage, rate and duration up to five years |
| `prop_only_an_approving_quorum_pays` | Every three-vote pattern either settles within the reserves or leaves reserves and lifetime payouts untouched |

## Running it

```bash
cargo test -p soroban-testkit-example-insurance
cargo clippy -p soroban-testkit-example-insurance --all-targets -- -D warnings
```
