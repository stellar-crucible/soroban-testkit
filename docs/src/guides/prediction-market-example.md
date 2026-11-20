# Worked Example: Prediction Market

A binary outcome market with oracle resolution: traders put collateral behind
YES or NO, the oracle reports a price after the market closes, and the side the
price favours redeems the whole pool. Everything here is real code in
[`examples/prediction_market`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/prediction_market),
a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/prediction_market</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">51 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-prediction-market</span></div>
</div>

## The contract

One [`#![no_std]`](https://doc.rust-lang.org/reference/attributes.html) contract
keeps every market in a persistent map and the next id in instance storage. A
market has two phases:

```text
Open ──oracle price──▶ Resolved ──dispute delay──▶ redeemable
```

```rust
#[contracttype]
pub enum MarketStatus {
    Open,
    Resolved,
}

pub fn resolve(env: Env, id: u64, oracle: Address, price: i128) {
    oracle.require_auth();

    let mut market = read_market(&env, id);
    assert!(market.status == MarketStatus::Open, "market has already been resolved");
    assert!(oracle == market.oracle, "caller is not the market oracle");
    assert!(env.ledger().timestamp() >= market.close_time, "market has not closed yet");
    assert!(price > 0, "oracle price must be positive");
    assert!(
        market.yes_shares > 0 && market.no_shares > 0,
        "both outcomes need collateral before resolution"
    );

    // ... set the winning side and unlock redemption dispute_delay later
}
```

Purchases are accepted only while the market is open, so nobody can buy after
seeing the oracle's report. Resolution is accepted only from the configured
oracle, only once, and only when both sides hold collateral — a one-sided market
has no losing collateral to pay the winners with, so settling it would strand
the pool.

Redemption is pro-rata. Each winning share is worth a slice of the entire pool,
and the last winner receives the remainder, so the payouts sum to exactly the
collateral the contract collected:

```rust
let payout = if shares >= remaining_shares {
    market.collateral_remaining
} else {
    shares * market.collateral_remaining / remaining_shares
};
```

The whole market record lives under one persistent key, so a purchase or a
redemption rewrites one entry rather than several. Removing a winner's position
in `claim` is what makes a second claim impossible without a separate
"already claimed" set.

## Why four files

The package splits its suite by crate, one file each, so a reader can open the
file for the crate they are learning and find nothing else in it.

| File | Crate | What it covers |
| --- | --- | --- |
| `tests/fixtures.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, funded traders, the full lifecycle, dispute delay and the rounding remainder |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `EventData`, `EventLog`, `assert_auth_matches!`, `AuthMatcher` |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | `budget_guard!`, `BudgetSnapshot`, `BudgetBaseline`, `StorageSnapshot`, `assert_storage_tier!` |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` over `token_amount()` and `contract_address()`, including the zero-collateral-loss invariant |

The three files under `tests/` are separate binaries and name the package
instead of the crate:

```rust
use soroban_testkit_example_prediction_market::{
    MarketStatus, Outcome, PredictionMarketContract, PredictionMarketContractClient,
};
```

## Fixtures — four traders, an oracle and a close time

`fixtures.rs` gives every role a stable index: 0 and 1 buy YES, 2 buys NO, and
index 5 is the oracle. Each trader is minted a starting balance, so a test reads
the final balance as "minted minus staked plus payout".

```rust
fn setup() -> MarketFixture {
    let ctx = TestContextBuilder::new().with_users(6).build();
    let contract = ctx.env.register(PredictionMarketContract, ());
    let token = ctx.env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();

    for index in 0..4 {
        let trader = ctx.user(index).clone();
        StellarAssetClient::new(&ctx.env, &token).mint(&trader, &10_000);
    }

    MarketFixture { ctx, contract, token, oracle: ctx.user(5).clone(), /* ... */ }
}
```

The lifecycle tests cover creation, accumulating purchases, buying either side,
the close-time cut-off, YES/NO resolution at and below the threshold, refusal of
a non-oracle and of a second resolution, redemption before and after the dispute
window, a losing claim, a repeated claim, and the remainder case. That last one
is the invariant in miniature: two YES holders of 1 and 2 shares with 3 shares
on NO split a pool of 6 as 2 and 4, not 1 and 2.

The clock is a fixture concept too. `CLOSE_TIME` is what the fixture's
`create` is built around, and a test moves past it with the env rather than
waiting:

```rust
fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME);
client.resolve(&id, &fixture.oracle, &100);
fixture.ctx.env.ledger().set_timestamp(CLOSE_TIME + DISPUTE_DELAY);
let payout = client.claim(&id, &fixture.trader(0));
```

## Assert — four event kinds and three authorisations

The contract publishes `market/created`, `market/bought`, `market/resolved` and
`market/redeemed`. Payload assertions read the struct as a map keyed by field
name:

```rust
EventMatcher::new(&fixture.ctx.env)
    .with_topic("resolved")
    .assert_data_matches(|data| {
        data.field("outcome")
            .and_then(|value| value.deserialize::<Outcome>())
            == Some(Outcome::Yes)
            && data.field("redeemable_at")
                .and_then(|value| value.deserialize::<u64>())
                == Some(CLOSE_TIME + DISPUTE_DELAY)
    });
```

`assert_not_emitted` is the negative half: a claim attempted inside the dispute
window fails and publishes no `redeemed` event, so a contract that emitted before
checking the clock would fail here. `EventLog` proves the order across the whole
lifecycle — `created`, `bought`, `bought`, `resolved`, `redeemed` — after
filtering out the token contract's own transfer events.

The file also names the invoker of each state change:

```rust
assert_auth_matches!(fixture.ctx.env, {
    invoker: fixture.oracle,
    function: "resolve",
    args: [id, _, 100i128],
});
```

`buy`, `resolve` and `claim` each record one authorisation, and a read records
none.

## Core — ceilings, one storage key, and a status transition

`budget_and_storage.rs` builds a bare `Env`, so it contains no other testkit
crate. The three mutating calls get ceilings, and crossing one fails with one
parseable line:

```text
BUDGET kind=ceiling case=create metric=cpu_insns actual=… limit=1
```

A read costs less than a purchase, and resolving costs more than reading the
market, which is the shape a benchmark expects rather than a machine-specific
number.

Storage is where the market's design shows. `create` writes `next_id` to
instance storage and `markets` to persistent storage. Every later transition
rewrites `markets` and leaves the counter alone — including resolution, whose
diff shows `Open` in the before value and `Resolved` in the after value.
Redemption reduces `collateral_remaining` and `winning_shares_remaining` to zero
while `pool` stays at the 150 the market collected, because the pool is history
rather than a balance. A refused purchase and a set of reads both leave the
snapshot unchanged.

```rust
soroban_testkit_core::assert_storage_tier!(
    &fixture.env, &fixture.contract, "next_id", StorageTier::Instance
);
soroban_testkit_core::assert_storage_tier!(
    &fixture.env, &fixture.contract, "markets", StorageTier::Persistent
);
```

## Generators — zero collateral loss

The property file's centrepiece is the rule the market exists to keep: the sum
of every payout equals the collateral collected.

```rust
proptest! {
    #[test]
    fn the_sum_of_payouts_equals_the_pool(
        yes_a in token_amount(),
        yes_b in token_amount(),
        no in token_amount(),
    ) {
        prop_assume!(yes_a > 0 && yes_b > 0 && no > 0);
        // ... buy both sides, close, resolve YES, unlock

        let first = client.claim(&id, &fixture.yes_one);
        let second = client.claim(&id, &fixture.yes_two);
        prop_assert_eq!(first + second, yes_a + yes_b + no);
        prop_assert_eq!(fixture.escrow(), 0);
    }
}
```

That is strict equality, not "at most": rounding cannot strand a stroop because
the final winner takes the remainder. The rest of the file covers the negative
space — a losing position is refused and keeps its shares, redemption inside the
dispute window always fails, the boundary price decides the outcome at exactly
the threshold, and purchases after the close time are refused.

`token_amount()` can generate zero and the market refuses zero purchases, so the
collateral properties `prop_assume!(amount > 0)`: a zero purchase is a different
property from the one each case is about. `contract_address()` supplies a valid
address that is not the oracle, so "only the oracle can resolve" is fuzzed
rather than restated with one hard-coded outsider.

<div class="tk-callout">
  <span class="tk-callout__title">Eight property cases, each with its own environment</span>
  <p>Every case registers the market and a Stellar asset contract and runs several calls on the host. The file keeps its case count at 48 so <code>cargo test --workspace</code> stays quick; each case builds its own <code>Env</code>, so no case inherits another's balances or clock.</p>
</div>

## Running it

```bash
cargo test -p soroban-testkit-example-prediction-market
```

```text
test a_winning_holder_claims_after_the_window_and_receives_the_pool ... ok
test two_winners_share_the_pool_with_the_remainder_going_to_the_last ... ok
                                        # tests/fixtures.rs — 19 tests

test the_lifecycle_events_arrive_in_order ... ok
test a_refused_redemption_publishes_no_redemption_event ... ok
                                        # tests/assert_events.rs — 11 tests

test resolution_transitions_the_status_in_the_stored_value ... ok
test a_refused_purchase_leaves_storage_untouched ... ok
                                        # tests/budget_and_storage.rs — 13 tests

test the_sum_of_payouts_equals_the_pool ... ok
test the_boundary_price_decides_the_outcome ... ok
                                        # tests/property.rs — 8 tests
```

51 tests in the package, and all of them inside `cargo test --workspace --locked`.

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">soroban-testkit-fixtures</h3>
    <p class="tk-card__body"><a href="../crates/fixtures.html">TestContext, the builder and the ledger clock</a> the fixture file is built on.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">soroban-testkit-assert</h3>
    <p class="tk-card__body"><a href="../crates/assert.html">Event and authorization matchers</a> for the four events and three signatures this market publishes.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Property testing</h3>
    <p class="tk-card__body"><a href="./property-testing.html">Strategies, cases and shrinking</a> behind the zero-collateral-loss invariant.</p>
  </div>
</div>
