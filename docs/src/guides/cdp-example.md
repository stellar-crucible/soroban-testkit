# Synthetic Asset & CDP Worked Example

This guide walks through `examples/cdp`, a protocol that mints a synthetic US dollar (`sUSD`) against XLM collateral. A user locks collateral in a collateralized debt position (CDP), borrows `sUSD` against it while staying above a minimum collateralization ratio (MCR), pays a stability fee on outstanding debt, and is liquidated if a price drop pushes them below the MCR.

The protocol needs several contracts working together, which makes it a good test of everything Testkit offers. The suite has 47 tests across 4 files and uses all four workspace crates.

## The contracts

| Contract | Module | Role |
|----------|--------|------|
| Collateral token | Stellar Asset Contract (tests) | XLM, locked into and released from the engine |
| `PriceOracle` | `oracle.rs` | Admin-fed USD price of the collateral, stamped with the ledger time |
| `SyntheticToken` | `synth.rs` | `sUSD`. Only its minter, the engine, can mint or burn |
| `CdpEngine` | `engine.rs` | Holds collateral, issues and tracks debt, accrues fees, liquidates |

All three are `#![no_std]`. Each lives in its own module so the generated clients and contract specs never collide. A deployment builds each one as a separate WASM.

```rust
use soroban_testkit_example_cdp::*;

let collateral = env.register_stellar_asset_contract_v2(admin.clone()).address();
let oracle_id = env.register(PriceOracle, ());
let synth_id = env.register(SyntheticToken, ());
let engine_id = env.register(CdpEngine, ());

PriceOracleClient::new(&env, &oracle_id).initialize(&admin);
SyntheticTokenClient::new(&env, &synth_id).initialize(&engine_id); // engine is the minter

CdpEngineClient::new(&env, &engine_id).initialize(&CdpConfig {
    admin: admin.clone(),
    collateral_token: collateral.clone(),
    synth_token: synth_id.clone(),
    oracle: oracle_id.clone(),
    mcr_bps: 15_000,                 // 150% minimum collateralization ratio
    liquidation_penalty_bps: 1_000,  // 10% bonus to the liquidator
    stability_fee_bps: 200,          // 2% a year on outstanding debt
    max_price_age: 3_600,            // refuse prices older than an hour
});
```

## Protocol mechanics

Prices are USD per collateral unit, scaled by `PRICE_SCALE = 10_000_000` (7 decimals, like Stellar assets). Ratios and rates are in basis points.

| Operation | Who signs | What happens |
|-----------|-----------|--------------|
| `deposit(owner, amount)` | owner | XLM moves owner → engine. The position's collateral grows |
| `borrow(owner, amount)` | owner | Fails with `BelowMinCollateralRatio` unless `collateral × price ≥ (debt + amount) × MCR`. Otherwise the engine has `sUSD` mint `amount` to the owner |
| `repay(owner, amount)` | owner | The engine burns `amount` of the owner's `sUSD` and reduces the debt |
| `withdraw(owner, amount)` | owner | XLM moves engine → owner, provided an indebted position stays at or above the MCR |
| `liquidate(liquidator, owner)` | liquidator | Only below the MCR. The liquidator burns the whole debt and receives collateral worth `debt × (1 + penalty)`, capped at what the position holds |
| `accrue(owner)` | anyone | Adds the stability fee accrued so far to the debt |

**MCR checks never round in the borrower's favour.** `is_healthy` compares `collateral × price × 10_000 ≥ debt × MCR × PRICE_SCALE` by cross-multiplying, so a position a hair below 150% is never truncated up to exactly 150%.

**Stability fee.** Debt grows by simple interest, `debt × fee_bps × elapsed / (10_000 × 31_536_000)`, and the fee is folded in whenever the position is touched. The accrual clock only moves when at least one whole unit of fee was added, so a small position touched often still pays. Fees are owed but never minted, so `total_debt − sUSD total_supply` is exactly the fee revenue accrued so far.

**Liquidation.** At a $0.08 price, 10,000 XLM ($800) backing 600 sUSD sits at 133%, below the MCR. A liquidator burns 600 sUSD and receives $660 of XLM (8,250 XLM). The owner keeps the 600 sUSD they borrowed and can withdraw the remaining 1,750 XLM. If the collateral is worth less than the debt, the liquidator takes all of it and the position is closed.

### Error conditions

| Error | Cause |
|-------|-------|
| `AlreadyInitialized` / `NotInitialized` | Engine initialized twice / used before initialization |
| `InvalidParameter` | MCR ≤ 100%, penalty or fee > 100%, or a zero `max_price_age` |
| `InvalidAmount` | Zero or negative amount |
| `NoPosition` | Borrow, repay, withdraw or liquidate against an owner with no position |
| `InsufficientCollateral` | Withdrawing more than the position holds |
| `BelowMinCollateralRatio` | A borrow or withdrawal would leave the position below the MCR |
| `ExceedsDebt` | Repaying more than is owed |
| `NotLiquidatable` | Liquidating a position at or above the MCR, or with no debt |
| `PriceUnavailable` / `StalePrice` | Oracle has no price / price is older than `max_price_age` |
| `InsufficientSynth` | Repayer or liquidator lacks the `sUSD` to burn |

## Test suite structure

```text
examples/cdp/tests/
├── common/mod.rs      # the `Cdp` fixture: deploys and wires all four contracts
├── fixtures.rs        # soroban-testkit-fixtures — lifecycle, MCR, fees, oracle, liquidation (22 tests)
├── assert.rs          # soroban-testkit-assert — events, auth, balance conservation (11 tests)
├── budget_storage.rs  # soroban-testkit-core — liquidation gas benchmark, budgets, storage (8 tests)
└── proptest.rs        # soroban-testkit-generators + proptest — MCR invariants (6 properties)
```

### The `Cdp` fixture (`tests/common/mod.rs`)

Built on `TestContextBuilder::new().with_users(3)`: the owner, a liquidator, and a second independent borrower. `Cdp::new()` deploys everything, publishes a $0.10 price, and funds each user with 1,000,000 XLM. Helpers keep the tests readable:

```rust
let mut cdp = Cdp::new();
cdp.open(&cdp.owner, 10_000 * ONE, 600 * ONE); // deposit + borrow
cdp.arm_liquidator(600 * ONE);                 // liquidator opens a safe CDP for sUSD
cdp.crash_price(20);                           // oracle drops 20%
cdp.advance_time(YEAR);                        // TestContext::advance_time + oracle refresh
cdp.advance_time_without_oracle(3_601);        // let the price go stale
```

### 1. Fixtures (`tests/fixtures.rs`)

Drives the whole lifecycle and pins exact numbers: a 600 sUSD borrow against $1,000 is a 16,666 bps ratio. `max_borrow` is exactly 666.6666666 sUSD, and one more stroop fails. A year at 2% turns 600 sUSD of debt into 612. Half a year leaves 6 sUSD of fees after repaying the principal. A 10% fee pushes a max-borrowed position below the MCR within a month. A stale or missing price blocks borrowing. A liquidator without enough `sUSD` is rejected, and nothing about the position changes.

### 2. Assertions (`tests/assert.rs`)

This is where the new [event count range macros](../crates/assert.md#event-count-ranges-and-topic-frequencies) are useful. Once time has passed, a `borrow` emits an extra `fee` event, so the same call publishes two events at `t = 0` and three a year later. An exact `assert_count(2)` would break. A range states what the test really depends on:

```rust
cdp.engine().borrow(&cdp.owner, &(100 * ONE));
assert_event_count_range!(env, 2..=3);
assert_event_topic_count!(env, "fee", 0);

cdp.advance_time(YEAR);
cdp.engine().borrow(&cdp.owner, &(100 * ONE));
assert_event_count_range!(env, 2..=3);          // still holds
assert_event_topic_count!(env, "fee", 1);       // the auxiliary event appeared
```

`from = …` restricts a count to one contract, which matters when several contracts emit a `transfer` topic:

```rust
cdp.engine().liquidate(&cdp.liquidator, &cdp.owner);
assert_event_topic_count!(env, from = cdp.engine_id, "liquidate", 1);
assert_event_topic_count!(env, from = cdp.synth_id, "burn", 1);
assert_event_topic_count!(env, from = cdp.collateral, "transfer", 1);
```

The macros also accept an `EventLog`, so one assertion can cover a whole deposit → borrow → borrow → repay → withdraw lifecycle (`assert_event_topic_count!(log, from = cdp.synth_id, "mint", 2)`). Other tests in this file read the `liquidate` payload with `assert_data_matches`, check signers with `assert_auth_matches!` (including argument patterns like `args: [_, 100 * ONE]`), and use `assert_balance_conserved!` to show that a liquidation only moves XLM between engine and liquidator and that a borrow mints exactly the requested `sUSD`.

### 3. Budget & storage (`tests/budget_storage.rs`)

**Liquidation gas benchmark.** A position opened at 160% is liquidated after price drops of 10%, 30%, 45%, 60%, and 90%, each under a `BudgetGuard` with CPU and memory ceilings. The test prints a table (`cargo test -p soroban-testkit-example-cdp --test budget_storage -- --nocapture`):

```text
| Price drop | Underwater | Seized (XLM) | Remaining (XLM) | CPU insns | Mem bytes |
|-----------:|:----------:|-------------:|----------------:|----------:|----------:|
|        10% |     no     |         7638 |            2361 |    387849 |     73771 |
|        30% |     no     |         9821 |             178 |    387849 |     73771 |
|        45% |    yes     |        10000 |               0 |    373723 |     71493 |
|        60% |    yes     |        10000 |               0 |    373723 |     71493 |
|        90% |    yes     |        10000 |               0 |    373723 |     71493 |
```

The test asserts that seizures grow with severity, that mild drops leave collateral behind and severe ones exhaust it, and that cost stays within 2× across all severities. Severity changes which branch runs, not how much work is done. An underwater liquidation is slightly cheaper because it deletes the position entry instead of rewriting it. (Exact numbers depend on the SDK version. The assertions do not.)

**Storage.** `StorageSnapshot` diffs show that opening a position adds exactly one persistent entry, borrowing rewrites it in place, and closing it (repay + withdraw) removes it. `assert_storage_tier_count!` pins one persistent entry per owner on the engine and one per holder on `sUSD`, and a zero balance is deleted rather than stored. `StorageFragmentationAnalyzer` produces a report over the engine's storage.

### 4. Properties (`tests/proptest.rs`)

Collateral and borrow sizes come from `soroban_testkit_generators::strategies::token_amount()`, and prices span $0.0001 to $10.

| Property | Invariant |
|----------|-----------|
| `prop_synthetic_is_never_minted_below_the_mcr` | A borrow succeeds **if and only if** the resulting position is at or above the MCR. A refused borrow mints nothing and records no debt |
| `prop_borrow_sequences_respect_the_mcr_at_every_step` | Across interleaved price moves and borrows, each accepted borrow leaves the position healthy, and supply = debt = sum of accepted borrows |
| `prop_max_borrow_is_always_mintable` | Borrowing exactly `max_borrow` always succeeds and stays at or above the MCR |
| `prop_withdraw_never_leaves_a_position_below_the_mcr` | A withdrawal succeeds exactly when the position stays healthy |
| `prop_liquidation_only_below_the_mcr_and_conserves_collateral` | Liquidation succeeds exactly below the MCR. Seized + remaining = original collateral, and the liquidator's gain equals the engine's loss |
| `prop_stability_fee_follows_simple_interest` | Accrued debt matches the simple-interest formula for any rate (0–20%) and elapsed time (0–5 years) |

## Running it

```bash
cargo test -p soroban-testkit-example-cdp
cargo test -p soroban-testkit-example-cdp --test budget_storage -- --nocapture  # benchmark table
```
