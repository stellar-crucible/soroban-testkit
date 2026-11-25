# Lending Worked Example

This guide walks through `examples/lending`, a money market built from four `#![no_std]` contracts: a collateral vault, a borrowing pool, a simple price oracle, and an internal debt token. A depositor locks a Stellar Asset Contract token, borrows debt tokens within a maximum loan-to-value ratio, watches the position's health factor move with the oracle price and ledger time, and can be liquidated by a third party.

The suite has 46 running tests across four files and uses all four workspace crates. The contracts are registered natively, which meters the host work without Wasm instantiation; the budget numbers are regression baselines rather than network fee estimates.

## The contracts

| Contract | Module | Role |
|----------|--------|------|
| Collateral token | Stellar Asset Contract (tests) | The asset users deposit and liquidators receive |
| `LendingOracle` | `oracle.rs` | Admin-fed price with the ledger timestamp it was published at |
| `DebtToken` | `debt_token.rs` | Internal token minted and burned only by the pool |
| `CollateralVault` | `vault.rs` | Holds collateral, enforces reserved collateral on withdrawal, allows the pool to seize |
| `BorrowingPool` | `pool.rs` | Tracks debt, checks LTV, accrues interest, and runs liquidation |

All four live in separate modules so the generated clients and contract specs do not collide. A deployment builds each one as its own Wasm.

```rust
use soroban_testkit_example_lending::*;

let collateral = env.register_stellar_asset_contract_v2(admin.clone()).address();
let oracle_id = env.register(LendingOracle, ());
let debt_id = env.register(DebtToken, ());
let vault_id = env.register(CollateralVault, ());
let pool_id = env.register(BorrowingPool, ());

LendingOracleClient::new(&env, &oracle_id).initialize(&admin);
DebtTokenClient::new(&env, &debt_id).initialize(&pool_id);

CollateralVaultClient::new(&env, &vault_id).initialize(&VaultConfig {
    pool: pool_id.clone(),
    collateral_token: collateral.clone(),
});
BorrowingPoolClient::new(&env, &pool_id).initialize(&PoolConfig {
    admin: admin.clone(),
    vault: vault_id,
    oracle: oracle_id,
    debt_token: debt_id,
    max_ltv_bps: 8_000,                 // borrow up to 80% of collateral value
    liquidation_threshold_bps: 11_000,  // liquidate below 110% health
    liquidation_penalty_bps: 500,       // 5% bonus to the liquidator
    interest_rate_bps: 500,             // 5% simple annual interest
    max_price_age: 63_072_000,          // roughly two years in this fixture
});
```

## Protocol mechanics

Prices are USD per collateral unit scaled by `PRICE_SCALE = 10_000_000` (seven decimals, like Stellar assets). Rates and ratios are basis points.

| Operation | Who signs | What happens |
|-----------|-----------|--------------|
| `deposit(owner, amount)` | owner | Collateral moves owner → vault and the position grows |
| `borrow(owner, amount)` | owner | Fails with `AboveMaxLtv` if `debt + amount` would exceed 80% of collateral value; otherwise the debt token mints `amount` to the owner |
| `withdraw(owner, amount)` | owner | Collateral moves vault → owner, provided what remains covers the outstanding debt at the current price |
| `repay(owner, amount)` | owner | Debt tokens are burned and the position's debt falls |
| `liquidate(liquidator, owner)` | liquidator | Only below the liquidation threshold. The liquidator burns the debt and receives collateral worth `debt × 1.05`, capped at what the position holds |
| `accrue(owner)` | anyone | Folds accrued interest into the stored debt position |

**LTV and health.** `max_borrow` computes `collateral × price / PRICE_SCALE × 8_000 / 10_000 − debt`. `health_factor_bps` computes `collateral × price × 10_000 / (PRICE_SCALE × debt)`; a debt-free position reports `i128::MAX`. With 10,000 collateral at $1 and 6,000 of debt the health factor is 16,666 bps. When the price drops to $0.65 it becomes 10,833 bps, below the 11,000 bps liquidation threshold.

**Interest.** Debt grows by simple interest, `debt × rate_bps × elapsed / (10_000 × 31_536_000)`. Queries such as `debt_of` and `health_factor` include the pending interest without writing it; `accrue`, `borrow`, `repay`, and `liquidate` fold it into storage and emit an `interest` event. A year at 5% turns 6,000 of debt into 6,300. Interest is owed but not minted, so debt-token supply still tracks principal.

**Liquidation.** The pool accrues interest, checks the threshold with a cross-multiplication that never rounds in the borrower's favour, burns the liquidator's debt tokens, and asks the vault to seize collateral. The vault only accepts a `seize` from the pool, so a caller cannot take another user's collateral directly. If collateral is worth less than the debt plus penalty, the liquidator takes all of it and the position is removed.

### Error conditions

| Error | Cause |
|-------|-------|
| `NotInitialized` / `AlreadyInitialized` | Contract used before setup / initialized twice |
| `InvalidAmount` | Zero or negative deposit, borrow, repay, withdraw, or seize |
| `InvalidParameter` | LTV at or above 100%, a threshold not above the LTV, or a zero parameter |
| `NoCollateral` / `NoPosition` | Borrowing without a deposit / mutating a position that does not exist |
| `AboveMaxLtv` | A borrow would exceed the ceiling |
| `InsufficientCollateral` / `ReservedCollateral` | Withdrawing too much / taking collateral needed for debt |
| `ExceedsDebt` | Repaying more than the position owes |
| `NotLiquidatable` | Liquidating a healthy or debt-free position |
| `PriceUnavailable` / `StalePrice` | Oracle has no price / its timestamp is older than `max_price_age` |
| `InsufficientDebtToken` | Liquidator or repayer lacks enough debt tokens to burn |

## Test suite structure

```text
examples/lending/tests/
├── common/mod.rs      # the `Lending` fixture: deploys and wires all four contracts
├── fixtures.rs        # soroban-testkit-fixtures — lifecycle, LTV, interest, oracle, liquidation (19 tests)
├── assert.rs          # soroban-testkit-assert — events, auth, balance conservation (10 tests)
├── budget_storage.rs  # soroban-testkit-core — baselines, ceilings, storage footprint (10 tests + 1 ignored recorder)
└── proptest.rs        # soroban-testkit-generators + proptest — seven properties at 64 cases
```

### The `Lending` fixture (`tests/common/mod.rs`)

Built on `TestContextBuilder::new().with_users(3)`: the owner, a keeper who can liquidate, and a second borrower. `Lending::new()` registers the four contracts, wires them, publishes a $1 price, and funds each actor with 1,000,000 units of collateral.

```rust
let mut lending = Lending::new();
lending.open(&lending.owner, 10_000 * ONE, 6_000 * ONE); // deposit + borrow
lending.arm_liquidator(6_000 * ONE);                     // keeper opens a safe position
lending.crash_price(35);                                 // oracle publishes a lower price
lending.advance_time(YEAR);                              // ledger clock + refreshed price
```

`advance_time_without_oracle` deliberately skips the refresh so staleness can be tested.

### 1. Fixtures (`tests/fixtures.rs`)

Drives the whole lifecycle and pins exact numbers. A 10,000-unit deposit supports 8,000 of borrowing at 80% LTV; one stroop more fails. Repayment frees collateral, but until then withdrawal is limited to what the debt does not reserve. A year at 5% makes 6,000 become 6,300, and a fall to $0.65 takes the health factor below the threshold. A third party then repays the debt and receives collateral plus the 5% penalty. Two borrowers keep isolated positions while the vault and pool totals add up.

### 2. Assertions (`tests/assert.rs`)

Event counts are scoped to the emitting contract, which matters when a borrow spans the pool's `borrow`, the debt token's `mint`, and the collateral token's `transfer`:

```rust
assert_event_topic_count!(env, from = lending.pool_id, "borrow", 1);
assert_event_topic_count!(env, from = lending.debt_id, "mint", 1);
assert_event_count_range!(env, 2);
```

`EventMatcher::assert_data_matches` reads what an event carried, including the `(debt, seized)` pair on `liquidate` and the interest amount. `EventLog` collects deposit, two borrows, repayment and withdrawal into one scope. `assert_auth_matches!` pins the owner on `deposit` and `repay` and the liquidator on `liquidate`. Two `assert_balance_conserved!` checks show that a liquidation only moves collateral between the vault and the keeper, and that a borrow mints exactly the requested debt-token supply.

### 3. Budget and storage (`tests/budget_storage.rs`)

Three hot paths are measured in fresh environments and compared with `examples/lending/budget.json`:

| Case | CPU instructions | Memory bytes |
|------|-----------------:|-------------:|
| `deposit` | 253,201 | 47,248 |
| `borrow` | 222,231 | 50,383 |
| `liquidate` | 481,056 | 97,552 |

The baseline test allows 10% growth and enforces absolute ceilings as well. `BudgetGuard::run` bounds each user-facing call, `BudgetSnapshot::diff` compares two invocations, and a read is shown to cost less than a write. Refresh the committed numbers deliberately with:

```bash
cargo test -p soroban-testkit-example-lending --test budget_storage -- --ignored budget_baseline_records_current_costs
```

On the storage side, `StorageSnapshot` diffs show that a deposit adds one persistent vault entry, a second borrow rewrites the existing pool entry, repayment and withdrawal remove their entries, and an underwater liquidation leaves only the keeper's positions. `assert_storage_tier!`, `assert_storage_tier_count!`, and `StorageFragmentationAnalyzer` pin where those entries live.

### 4. Properties (`tests/proptest.rs`)

Collateral and interest rates come from `soroban_testkit_generators::financial`, borrow requests from `strategies::token_amount()`, and prices span $0.0001 to $10. Every property runs 64 cases.

| Property | Invariant |
|----------|-----------|
| `prop_a_borrow_never_exceeds_the_ltv_ceiling` | A borrow succeeds only up to `max_borrow`; a refused borrow mints nothing |
| `prop_a_lower_price_never_raises_the_health_factor` | A lower oracle price cannot increase health, and the query matches the pure helper |
| `prop_max_borrow_is_always_safe_to_take` | Borrowing exactly the ceiling succeeds and leaves no further capacity |
| `prop_withdrawal_never_releases_reserved_collateral` | A withdrawal succeeds exactly when the remaining collateral still covers the debt |
| `prop_interest_follows_simple_interest_at_any_rate` | Accrued debt matches the simple-interest formula for rates from 0 to 100% and up to five years |
| `prop_liquidation_only_below_threshold_and_conserves_collateral` | Liquidation happens only below the threshold, and the liquidator's collateral gain equals the vault's loss |
| `prop_two_borrowers_have_additive_totals` | Two borrowers' debt, supply, and collateral sum to the protocol totals |

## Running it

```bash
cargo test -p soroban-testkit-example-lending
cargo clippy -p soroban-testkit-example-lending --all-targets --locked -- -D warnings
```
