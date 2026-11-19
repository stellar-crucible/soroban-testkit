# Worked Example: Stablecoin

A multi-contract DeFi protocol and its 38-test suite showing how every crate in
`soroban-testkit` composes on a realistic over-collateralized stablecoin:
multi-asset vaults, Stability Pool debt absorption, algorithmic redemption fees
that decay with the ledger clock, oracle-driven liquidations, and multi-hop
router authorizations. Everything here is real code in
[`examples/stablecoin`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/stablecoin),
a `#![no_std]` workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/stablecoin</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">38 passing (+ 2 baseline runners)</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-stablecoin</span></div>
</div>

## The contract suite

Three `#![no_std]` contracts live in `examples/stablecoin/src/lib.rs`:

- **`PriceOracleContract`** — admin-gated USD price feed (7-decimal `PRICE_SCALE`)
  storing per-asset prices in persistent storage and emitting `PriceUpdated`.
- **`StablecoinContract`** — multi-collateral vault engine enforcing per-asset
  minimum collateralization ratios (`asset_mcr_bps >= mcr_bps`), Stability Pool
  staking and liquidation debt absorption, and Liquity-style algorithmic
  redemption fees whose base rate rises with redemption volume and decays over
  ledger time.
- **`VaultRouterContract`** — multi-hop router combining `deposit_collateral`
  and `mint` in one atomic call so the test suite can validate nested
  authorization sub-trees with `assert_auth_sub_tree_matches!`.

```rust
pub fn liquidate(env: Env, liquidator: Address, borrower: Address, asset: Symbol) -> i128 {
    liquidator.require_auth();

    let asset_mcr = Self::asset_mcr_bps(env.clone(), asset.clone());
    let price = fetch_oracle_price(&env, &asset);

    let mut vaults = vaults_map(&env);
    let key = (borrower.clone(), asset.clone());
    let position = vaults.get(key.clone()).unwrap_or_else(empty_vault);
    assert!(position.debt > 0, "vault has no debt");
    assert!(
        !is_healthy(position.collateral, position.debt, price, asset_mcr),
        "vault is healthy"
    );
    // Absorbs borrower debt from the Stability Pool and transfers collateral
    // into the pool reserve.
}
```

## Four dedicated test files

Following the same one-crate-per-file layout as
[`Worked Example: Token`](./token-example.md), each file isolates one workspace
crate:

| File | Crate | What it covers |
| --- | --- | --- |
| `src/test.rs` | `soroban-testkit-fixtures` | Multi-asset vaults, MCR checks, Stability Pool liquidations, `advance_time` fee decay, `reset` |
| `tests/assert_events.rs` | `soroban-testkit-assert` | Multi-contract `from_contract`, `EventLog`, `assert_auth_matches!`, `assert_auth_sub_tree_matches!` |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | Mint and liquidation `budget_guard!`, committed `budget.json` baselines, Instance/Persistent/Temporary tiers |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` proving strict global MCR backing, conservation, fee bounds, and liquidations |

## Fixtures — multi-actor lifecycle and clock-driven fee decay

`src/test.rs` provisions borrowers, Stability Pool stakers, and liquidators
with `TestContextBuilder::new().with_users(n).build()`. Algorithmic redemption
fees decay by `1` bps per hour (`FEE_DECAY_WINDOW_SECS = 3_600`), so
`ctx.advance_time(...)` tests the decay directly:

```rust
let collateral_out = proto.stablecoin.redeem(&redeemer, &borrower, &xlm, &100);
assert_eq!(collateral_out, 97);
assert_eq!(proto.stablecoin.base_rate_bps(), 250);

// Advance ledger time by 100 hours: base rate decays by 100 bps.
ctx.advance_time(FEE_DECAY_WINDOW_SECS * 100);
assert_eq!(proto.stablecoin.base_rate_bps(), 150);
```

## Assert — multi-contract events and sub-tree authorizations

With `PriceOracleContract`, `StablecoinContract`, and `VaultRouterContract`
registered in one environment, `EventMatcher::from_contract` keeps oracle price
updates separate from vault events, while `EventLog` captures the entire
`deposit -> mint -> sp_stake -> price -> liquidate` lifecycle in order.

`VaultRouterContract::deposit_and_mint` calls both `deposit_collateral` and
`mint` inside one invocation, exercising `assert_auth_sub_tree_matches!` on
nested authorization trees:

```rust
d.router.deposit_and_mint(&d.stable_id, &borrower, &xlm, &1_500, &1_000);

assert_auth_sub_tree_matches!(ctx.env, {
    invoker: borrower,
    contract: d.router_id,
    function: "deposit_and_mint",
    sub_invocations: [
        {
            contract: d.stable_id,
            function: "deposit_collateral",
            args: [_, xlm.clone(), 1_500i128],
            sub_invocations: [],
        },
        {
            contract: d.stable_id,
            function: "mint",
            args: [_, xlm.clone(), 1_000i128],
            sub_invocations: [],
        },
    ],
});
```

## Core — budget baselines and all three storage tiers

`tests/budget_and_storage.rs` benchmarks `mint` and `liquidate` with
`budget_guard!`, compares read vs. liquidation cost with
`BudgetSnapshot::last_invocation`, and loads the committed baseline in
`examples/stablecoin/budget.json` covering `deposit_collateral`, `liquidate`,
`mint`, and `redeem`.

The contract intentionally spans all three Soroban storage tiers so
`assert_storage_tier!` and `StorageSnapshot` can pin every key:

| Tier | Keys | Purpose |
| --- | --- | --- |
| `Instance` | `admin`, `oracle`, `mcr_bps`, `supply`, `sp_total`, `base_rate`, `last_fee_at` | Protocol parameters and global counters |
| `Persistent` | `assets`, `balances`, `vaults`, `pool_stakes`, `pool_collateral` | Per-user positions and Stability Pool reserves |
| `Temporary` | `cache_liq_ts`, `cache_fee_bps` | Ephemeral liquidation and redemption telemetry |

```rust
soroban_testkit_core::assert_storage_tier!(
    &proto.env,
    &proto.stable_id,
    "pool_*",
    StorageTier::Persistent
);
soroban_testkit_core::assert_storage_tier!(
    &proto.env,
    &proto.stable_id,
    "cache_*",
    StorageTier::Temporary
);
```

To re-record or check drift against `examples/stablecoin/budget.json` on a
pinned runner:

```bash
cargo test --locked -p soroban-testkit-example-stablecoin -- --ignored --nocapture budget_baseline_records_current_costs
cargo test --locked -p soroban-testkit-example-stablecoin -- --ignored --nocapture budget_baseline_rejects_drifted_costs
```

## Generators — proving global MCR solvency

`tests/property.rs` runs 7 `proptest!` suites across `account_address()`,
`contract_address()`, `address()`, `token_amount()`, `ledger_sequence()`, and
`timestamp()`. The flagship property verifies that across arbitrary multi-asset
collateral deposits, prices, utilization levels, and repayments, total
circulating stablecoin supply is strictly backed by collateral above MCR:

```rust
let backing_a = vault_a.collateral * xlm_price / PRICE_SCALE;
let backing_b = vault_b.collateral * wbtc_price / PRICE_SCALE;
let total_backing = backing_a + backing_b;

prop_assert_eq!(total_supply, vault_a.debt + vault_b.debt);
prop_assert!(total_backing * BPS_SCALE >= total_supply * DEFAULT_MCR_BPS);
```

## Running it

```bash
cargo test -p soroban-testkit-example-stablecoin
```

All 38 tests run as part of `cargo test --workspace --locked`, and the two
baseline runner tests are `#[ignore]`d for deliberate recording and pinned-runner
drift checks.

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">soroban-testkit-assert</h3>
    <p class="tk-card__body"><a href="../crates/assert.html">Sub-tree authorization matching</a> used by the router tests.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Budget-Aware Testing</h3>
    <p class="tk-card__body"><a href="./budget-testing.html">Ceilings, baselines, and CI drift workflows</a> behind <code>budget.json</code>.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Worked Example: Token</h3>
    <p class="tk-card__body"><a href="./token-example.html">The single-contract ERC-20 example</a> that introduced the four-file layout.</p>
  </div>
</div>
