# Worked Example: Order Book DEX with Price-Time Priority

A Central Limit Order Book (CLOB) decentralized exchange on Soroban: makers place resting limit orders with escrowed collateral, bids and asks maintain strict price-time priority ordering, and market orders execute with deterministic partial and full fill settlements. Everything here is real code in [`examples/orderbook`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/orderbook), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/orderbook</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">29 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-orderbook</span></div>
</div>

## The contract

One [`#![no_std]`](https://doc.rust-lang.org/reference/attributes.html) contract manages resting order queues, price-time sorting, collateral escrow, and market execution matching:

```text
Maker Limit Order ──▶ Escrow Collateral ──▶ Sorted Queue (Bids/Asks) ──▶ Market Match ──▶ Settlement & Deletion
```

### Price-time priority queues

The order book maintains two queues of resting orders:
- **Bids (Buy Orders)**: Sorted by price **descending** (highest buy price first). For equal prices, orders are sorted by timestamp and ID ascending (earliest first).
- **Asks (Sell Orders)**: Sorted by price **ascending** (lowest sell price first). For equal prices, orders are sorted by timestamp and ID ascending.

```rust
// Bids sorted descending: highest price first
if !inserted && price > existing_order.price {
    new_bids.push_back(order_id);
    inserted = true;
}
```

### Maker collateral escrow & cancellations

When a maker places an order:
- A `Buy` order escrows `(amount * price) / PRICE_PRECISION` quote tokens in the contract.
- A `Sell` order escrows `amount` base tokens in the contract.

When an order is cancelled or partially filled:
- Unfilled collateral is refunded to the maker immediately.
- The order entry is completely removed from storage upon cancellation or 100% execution, reclaiming contract footprint and preventing state bloat.

### Partial fill execution

When a taker executes a market order:
- The contract walks through the resting book up to the requested volume.
- Matched volume is executed at the resting maker's price.
- If an order is partially filled, its `filled` tally updates and the order remains in the book.
- If an order is fully filled (`filled == amount`), it is pruned from the queue and deleted from persistent storage.

## Why four files

The test suite divides its coverage cleanly across four dedicated files, exercising all four workspace testkit crates:

| File | Crate | What it covers |
| --- | --- | --- |
| `tests/fixtures.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, user funding, token setup, order placement, order cancellation, refunds, full matching, and partial matching |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventLog`, `assert_auth_matches!`, `assert_event_count_range!`, `assert_event_topic_count!`, and negative event assertions |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | `BudgetSnapshot`, `StorageSnapshot`, instruction cost scaling across matching depths (1, 5, 10), and storage diff proving order deletion |
| `tests/property.rs` | `soroban-testkit-generators` | Proptest strategies for price and amounts, verifying that buyer received tokens and seller received payment matching the price |

## Key test patterns

### Price-time priority verification

`tests/fixtures.rs` asserts that orders placed out of order automatically sort into their correct priority slots:

```rust
#[test]
fn place_orders_maintains_bid_price_priority_descending() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.ctx.env, &fx.book);

    let id1 = client.place_order(&fx.maker_1, &Side::Buy, &(2 * PRICE_PRECISION), &(10 * ONE));
    let id2 = client.place_order(&fx.maker_1, &Side::Buy, &(3 * PRICE_PRECISION), &(10 * ONE));
    let id3 = client.place_order(&fx.maker_2, &Side::Buy, &(25 * PRICE_PRECISION / 10), &(10 * ONE));

    let bids = client.get_bids();
    assert_eq!(bids.get(0).unwrap(), id2); // 3.0
    assert_eq!(bids.get(1).unwrap(), id3); // 2.5
    assert_eq!(bids.get(2).unwrap(), id1); // 2.0
}
```

### Storage deletion proof

`tests/budget_and_storage.rs` captures contract storage before order creation, during resting state, and after full execution, proving with `StorageSnapshot` that filled entries leave zero lingering storage footprint:

```rust
#[test]
fn storage_diff_proves_order_deletion_upon_full_fill() {
    let fx = setup();
    let client = OrderBookContractClient::new(&fx.env, &fx.book);

    let initial_storage = StorageSnapshot::capture(&fx.env, &fx.book);
    let order_id = client.place_order(&fx.maker, &Side::Sell, &(2 * PRICE_PRECISION), &(10 * ONE));

    // Storage increases while order is resting
    assert!(StorageSnapshot::capture(&fx.env, &fx.book).len() > initial_storage.len());

    // 100% filled market order
    client.execute_market_order(&fx.taker, &Side::Buy, &(10 * ONE));
    let filled_storage = StorageSnapshot::capture(&fx.env, &fx.book);

    // Order entry is fully erased from persistent storage
    assert_eq!(filled_storage.len(), initial_storage.len());
    assert!(client.get_order(&order_id).is_none());
}
```

### Matching depth CPU instruction benchmarking

`tests/budget_and_storage.rs` compares CPU instruction consumption across execution depths of 1, 5, and 10 matched orders, verifying predictable linear budget growth:

```rust
assert!(depth_1_cost.cpu_insns < depth_5_cost.cpu_insns);
assert!(depth_5_cost.cpu_insns < depth_10_cost.cpu_insns);
```
