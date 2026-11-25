# Worked Example: Dynamic AMM with Volatility Fees

An Automated Market Maker (AMM) with volatility-adjusted dynamic fees: liquidity providers deposit token pairs, a time-weighted average price (TWAP) accumulator tracks price movement across ledger blocks, and swap fees automatically scale up during periods of high price volatility to protect LPs against impermanent loss and toxic order flow. Everything here is real code in [`examples/dynamic_amm`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/dynamic_amm), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/dynamic_amm</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">34 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-dynamic-amm</span></div>
</div>

## The contract

One [`#![no_std]`](https://doc.rust-lang.org/reference/attributes.html) contract manages pool reserves, LP shares, TWAP accumulators, and dynamic swap fee calculations.

```text
Deposit A + B ──▶ Mint LP Shares ──▶ Swaps adjust Spot Price ──▶ TWAP accumulates over time ──▶ Dynamic Fee Scales
```

### Constant product & spot price

The pool maintains reserves of two tokens, `token_a` and `token_b`. The instantaneous spot price expresses the ratio of reserves scaled to 7 decimals (`PRICE_PRECISION = 10_000_000`):

```rust
pub fn calculate_spot_price(reserve_a: i128, reserve_b: i128) -> Option<i128> {
    if reserve_a <= 0 || reserve_b <= 0 {
        return None;
    }
    reserve_b
        .checked_mul(PRICE_PRECISION)?
        .checked_div(reserve_a)
}
```

### Time-Weighted Average Price (TWAP)

To measure genuine pool trends without being fooled by transient flash loans or single-block manipulation, the pool records a cumulative price accumulator across ledger blocks:

$$\text{price\_cumulative} \mathrel{+}= \text{spot\_price} \times \Delta t$$

When the elapsed time reaches the configured observation window (`twap_window`, e.g. 300 seconds), the TWAP is updated:

$$\text{TWAP} = \frac{\Delta \text{price\_cumulative}}{\Delta t_{\text{window}}}$$

```rust
let time_elapsed = now.saturating_sub(twap.last_timestamp);
if time_elapsed > 0 && twap.last_spot_price > 0 {
    let incremental = twap.last_spot_price * (time_elapsed as i128);
    twap.price_cumulative += incremental;
    twap.last_timestamp = now;

    let window_elapsed = now.saturating_sub(twap.window_start_timestamp);
    if window_elapsed >= config.twap_window {
        twap.twap_price = (twap.price_cumulative - twap.window_start_price_cumulative)
            / (window_elapsed as i128);
        twap.window_start_timestamp = now;
        twap.window_start_price_cumulative = twap.price_cumulative;
    }
}
```

### Volatility-adjusted dynamic fee

When large swaps cause the current spot price to diverge significantly from the historical TWAP, the pool experiences high volatility. The fee scales up with deviation in basis points, strictly clamped between `min_fee_bps` and `max_fee_bps`:

```rust
pub fn calculate_dynamic_fee_bps(
    spot_price: i128,
    twap_price: i128,
    base_fee_bps: u32,
    min_fee_bps: u32,
    max_fee_bps: u32,
    volatility_factor: u32,
) -> u32 {
    if spot_price <= 0 || twap_price <= 0 {
        return base_fee_bps.clamp(min_fee_bps, max_fee_bps);
    }

    let diff = if spot_price >= twap_price {
        spot_price - twap_price
    } else {
        twap_price - spot_price
    };

    let deviation_bps = (diff * BPS_DENOMINATOR / twap_price) as u32;
    let extra_fee = deviation_bps.saturating_mul(volatility_factor);
    let raw_fee = base_fee_bps.saturating_add(extra_fee);
    raw_fee.clamp(min_fee_bps, max_fee_bps)
}
```

During quiet periods ($\text{spot} \approx \text{TWAP}$), trades enjoy standard fees (e.g. 30 bps = 0.30%). During sharp movements, fees rise up to `max_fee_bps` (e.g. 500 bps = 5.00%), capturing arbitrageur rent for liquidity providers.

## Why four files

The test suite divides its coverage cleanly across four dedicated files, exercising all four workspace testkit crates:

| File | Crate | What it covers |
| --- | --- | --- |
| `tests/fixtures.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, user funding, pool initialization, LP share minting/burning, multi-block TWAP updates, and fee cooling |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `EventLog`, `assert_event_topic_count!`, `assert_auth_matches!`, and intermediate token transfer isolation with `assert_auth_sub_invocation!` |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` & `assert` | Benchmarking dynamic fee overhead against constant product AMM, `budget_guard!`, `BudgetSnapshot`, `assert_storage_tier_count!` |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` proving fee clamping within $[min, max]$, constant-product invariant $k_{\text{after}} \ge k_{\text{before}}$, and monotonic fee scaling |

## Fixtures — pool lifecycle and multi-block TWAP updates

`tests/fixtures.rs` uses `TestContextBuilder` to set up funded accounts and test pool operations:

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;

let ctx = TestContextBuilder::new().with_users(3).build();
let pool = ctx.env.register(DynamicAmmContract, ());
```

Key lifecycle tests:
- **Initial liquidity**: mints $S = \sqrt{A \times B}$ LP shares.
- **Proportional provisioning**: second depositor receives shares exactly proportional to current reserves.
- **Partial and complete withdrawals**: burns shares and returns underlying assets.
- **TWAP across ledger blocks**: uses `ctx.env.ledger().set_timestamp(...)` to simulate time passage across blocks, proving price accumulation and window calculation.
- **Dynamic fee scaling and cooling**: demonstrates that fees rise during large swaps and cool down as time passes and the pool stabilizes.

## Event and authorization assertions

`tests/assert_events.rs` uses `soroban-testkit-assert` to verify the public observability and authorization requirements of AMM operations:

```rust
use soroban_testkit_assert::events::{EventLog, EventMatcher};
use soroban_testkit_assert::{assert_auth_matches, assert_auth_sub_invocation, assert_event_topic_count};

// Verify swap event emission
EventMatcher::new(&fx.ctx.env)
    .from_contract(&fx.pool)
    .with_topic("swap")
    .assert_emitted();

// Assert direct swapper authorization
assert_auth_matches!(fx.ctx.env, {
    invoker: fx.trader,
    function: "swap",
    args: [fx.trader.clone(), fx.token_a.clone(), 5_000 * ONE, 1i128],
});

// Isolate intermediate token transfer sub-invocation using assert_auth_sub_invocation!
assert_auth_sub_invocation!(fx.ctx.env, {
    contract: fx.token_a,
    function: "transfer",
    args: [fx.trader.clone(), fx.pool.clone(), 5_000 * ONE],
});
```

Using `assert_auth_sub_invocation!`, tests isolate the intermediate token contract transfer without needing to assert on top-level invoker wrappers or entire sub-trees.

## Budget benchmarking & storage quotas

`tests/budget_and_storage.rs` evaluates the performance and gas overhead of the dynamic fee mechanism compared to standard constant-product AMMs:

```rust
use soroban_testkit_core::budget::BudgetSnapshot;

// Constant-fee baseline swap
client1.swap_constant_fee(&fx1.trader, &fx1.token_a, &(5_000 * ONE), &1, &30);
let constant_cost = BudgetSnapshot::last_invocation(&fx1.env);

// Dynamic-fee swap with TWAP calculation
client2.swap(&fx2.trader, &fx2.token_a, &(5_000 * ONE), &1);
let dynamic_cost = BudgetSnapshot::last_invocation(&fx2.env);

// The dynamic fee calculation overhead is strictly bounded (< 30%)
let cpu_overhead_pct = ((dynamic_cost.cpu_insns as f64 - constant_cost.cpu_insns as f64)
    / constant_cost.cpu_insns as f64) * 100.0;
assert!(cpu_overhead_pct < 30.0);
```

Storage tier allocations are enforced using `assert_storage_tier_count!`:

```rust
use soroban_testkit_assert::assert_storage_tier_count;

// Instance storage holds Config, Reserves, TotalShares, and Twap
// Persistent storage holds user LP shares
assert_storage_tier_count!(fx.env, fx.pool, {
    instance: 4,
    persistent: 1,
    temporary: 0,
});
```

## Property testing

`tests/property.rs` uses `soroban-testkit-generators` and `proptest!` to prove critical DeFi invariants across arbitrary values:

1. **Strict fee bounds**: Proves that for all generated spot prices, TWAP values, and volatility multipliers, $\text{fee} \in [\text{min\_fee\_bps}, \text{max\_fee\_bps}]$.
2. **Monotonic fee scaling**: Proves that greater price deviation from TWAP always yields equal or higher fees.
3. **Zero deviation base fee**: When $\text{spot} = \text{TWAP}$, fee strictly equals `base_fee_bps`.
4. **Constant product invariant**: Across arbitrary reserve ratios and swap amounts, $k_{\text{after}} \ge k_{\text{before}}$ holds with zero precision loss.
5. **Solvency bound**: Output amount is strictly less than reserve out ($\text{amount\_out} < \text{reserve\_out}$).
6. **Babylonian square root accuracy**: $\lfloor\sqrt{y}\rfloor^2 \le y < (\lfloor\sqrt{y}\rfloor + 1)^2$.

```rust
proptest! {
    #[test]
    fn proptest_dynamic_fee_strictly_within_min_and_max_bounds(
        spot_price in 1i128..=10_000_000_000_000i128,
        twap_price in 1i128..=10_000_000_000_000i128,
        min_fee in 1u32..=100u32,
        base_fee_offset in 0u32..=200u32,
        max_fee_offset in 0u32..=2000u32,
        volatility_factor in 0u32..=20u32,
    ) {
        let base_fee = min_fee + base_fee_offset;
        let max_fee = base_fee + max_fee_offset;

        let fee = calculate_dynamic_fee_bps(
            spot_price,
            twap_price,
            base_fee,
            min_fee,
            max_fee,
            volatility_factor,
        );

        prop_assert!(fee >= min_fee);
        prop_assert!(fee <= max_fee);
    }
}
```
