# Dutch and Batch Auction Worked Example

This guide walks through `examples/auction`, a single `#![no_std]` contract, `AuctionHouse`, that runs two kinds of auction:

- **Dutch auction.** One lot is offered at a price that *falls* over ledger time. The first bidder willing to pay the current asking price buys the whole lot.
- **Batch auction.** A supply of units is offered for a bidding window. Bids are collected and then evaluated *together*: they are ranked, filled from the supply, and every winner pays the same uniform *clearing price*.

Both kinds sell units of an `asset` token for a `payment` token, and both escrow everything in advance, so settlement only ever pays out of funds the contract already holds. The suite has 55 tests (plus one deliberately ignored baseline recorder) in four files, and all four workspace crates are used in the clearing scenarios.

## Dutch vs batch, in one table

| | Dutch | Batch |
|---|---|---|
| What is sold | One lot of `lot_size` units, as a whole | `supply` units, split across bidders |
| Price | Asking price for the lot falls from `start_price` to `floor_price` | Discovered at settlement: the uniform clearing price per unit |
| Who wins | The first bid at or above the asking price | The highest-priced bids, until supply runs out |
| What a winner pays | The asking price when they bid (not their maximum) | `allocation × clearing_price`, never above their own maximum |
| Bidding window | `start_time ≤ now ≤ start_time + duration` | `start_time ≤ now < end_time` |
| Escrow per bid | The asking price | `quantity × max_price` |
| Payout | `settle` pays the buyer and the seller | `settle` pays the seller, then each bidder `claim`s units and refund |

## Architecture and entry points

```text
examples/auction/
├── src/lib.rs          # AuctionHouse contract, types, errors, events
├── budget.json         # committed budget baseline for the benchmark
└── tests/
    ├── common/mod.rs   # the `House` fixture, built on soroban-testkit-fixtures
    ├── fixtures.rs     # soroban-testkit-fixtures — lifecycle and clearing (19 tests)
    ├── assert.rs       # soroban-testkit-assert — errors, events, auth, conservation (22 tests)
    ├── budget_storage.rs # soroban-testkit-core — benchmark, budgets, limits, storage (8 + 1 ignored)
    └── proptest.rs     # soroban-testkit-generators + proptest — invariants (6 properties)
```

| Entry point | Signs | What it does |
|-------------|-------|--------------|
| `create_dutch(seller, DutchConfig) -> u64` | seller | Checks the schedule, escrows `lot_size` asset units, returns the auction id |
| `create_batch(seller, BatchConfig) -> u64` | seller | Checks supply, reserve and window, escrows `supply` asset units |
| `bid_dutch(id, bidder, max_price) -> i128` | bidder | Buys the lot if `max_price ≥` the current asking price, escrowing that price |
| `bid_batch(id, bidder, quantity, max_price)` | bidder | Places the bidder's one bid, escrowing `quantity × max_price` |
| `settle(id) -> Auction` | anyone | Settles once: Dutch delivers lot and proceeds; batch discovers the clearing price and pays the seller |
| `claim(id, bidder) -> Bid` | anyone | After batch settlement, sends the bidder their allocation and refund, once |
| `auction(id)`, `current_price(id)`, `get_bid(id, bidder)`, `book(id)` | — | Read-only views. `get_bid` shows a bid's allocation, payment and refund as soon as the auction is settled |

Storage: the id counter lives in instance storage. Each auction (`[Auction, id]`), its ranked order book (`[Book, id]`) and each batch bid (`[Bid, id, bidder]`) are persistent entries.

## Configuring and initializing an auction

```rust
use soroban_testkit_example_auction::{AuctionHouse, AuctionHouseClient, BatchConfig, DutchConfig};

let client = AuctionHouseClient::new(&env, &env.register(AuctionHouse, ()));

// A 100-unit lot asking 10_000, falling to 4_000 over 600 s in 1 s steps.
let dutch = client.create_dutch(&seller, &DutchConfig {
    asset: asset.clone(),
    payment: payment.clone(),
    lot_size: 100,
    start_price: 10_000,
    floor_price: 4_000,
    start_time: now,
    duration: 600,
    step_seconds: 1,
});

// 1_000 units, at least 10 per unit, bids accepted for an hour.
let batch = client.create_batch(&seller, &BatchConfig {
    asset: asset.clone(),
    payment: payment.clone(),
    supply: 1_000,
    reserve_price: 10,
    start_time: now,
    end_time: now + 3_600,
});
```

`create_dutch` rejects with `InvalidConfig` a non-positive lot or floor, a floor above the start price, a zero duration or step, a step longer than the duration, or a duration that is not a multiple of the step (so the floor is always reached exactly at the end). If `(start_price − floor_price) × duration` would overflow an `i128`, it rejects with `ArithmeticOverflow`. That check is what guarantees the price calculation can never overflow later. `create_batch` rejects a non-positive supply or reserve, and an empty window.

## Bidding and escrow

Every bid needs the bidder's authorization, and the payment is moved into the contract in the same call through the payment token's `transfer`. If the bidder cannot cover it, the token contract fails the call and the whole invocation rolls back: no bid is recorded, no order book entry is added, and no funds move.

<div class="tk-callout">
  <span class="tk-callout__title">Error codes start at 101</span>
  <p>A failure inside the token contract propagates through the escrow call unchanged. An insufficient balance arrives at the caller as the Stellar Asset Contract's own <code>Error(Contract, #10)</code>. If <code>AuctionError</code> were numbered from 1, a client would decode that as auction error 10. Starting at 101 keeps the two apart, and <code>tests/assert.rs</code> asserts the token error explicitly with <code>assert_host_error!(…, Error::from_contract_error(10))</code>.</p>
</div>

Batch bids also enforce these rules:

- one bid per bidder (`DuplicateBid`);
- `quantity > 0` (`InvalidQuantity`);
- `max_price ≥ reserve_price` (`BidBelowPrice`);
- the seller cannot bid (`SellerCannotBid`);
- at most `MAX_BATCH_BIDS = 64` bids per auction (`TooManyBids`);
- the escrow `quantity × max_price` must fit in an `i128`, checked before any transfer (`ArithmeticOverflow`).

## Dutch price decay

```text
elapsed  = clamp(now − start_time, 0, duration)
stepped  = elapsed − elapsed mod step_seconds
drop     = (start_price − floor_price) × stepped / duration     (integer division, rounds down)
asking   = start_price − drop                                   (so the price rounds up)
```

With the defaults above the price falls 10 per second: 10,000 at the start, 8,500 after 150 s, 7,000 after 300 s, and 4,000 from 600 s on. With `step_seconds = 60` it holds at 10,000 for the first minute and then drops 600 at once. Rounding the drop down means the asking price never undercuts the published schedule and never goes below the floor. A 999-unit spread over 7 s asks 858 after one second, not 857.

`bid_dutch` accepts the first bid with `max_price ≥ asking` while the window is open. It escrows the **asking price**, not the bidder's maximum, records the winner, and refuses every later bid with `LotAlreadySold`. The tests use `TestContext::advance_time` to step the ledger clock:

```rust
let id = house.dutch();
assert!(house.client().try_bid_dutch(&id, &bidder, &8_000).is_err()); // asking 10_000
house.advance(200);                                                   // asking 8_000
assert_eq!(house.client().bid_dutch(&id, &bidder, &8_000), 8_000);
```

`settle` delivers the lot to the buyer and the escrowed price to the seller. If the window closes with no buyer, `settle` returns the lot to the seller. Settling before then fails with `BiddingOpen`, and settling twice fails with `AlreadySettled`.

## Batch clearing-price discovery

The order book is kept **ranked as bids arrive**: by `max_price` descending, then by submission order (earlier first). Settlement works through it greedily:

1. **Undersubscribed** (total demand ≤ supply). Every bid is filled in full, and the clearing price is the **reserve price**. Demand exactly equal to supply counts as undersubscribed.
2. **Oversubscribed** (total demand > supply). Bids are filled in rank order until the supply is gone. The clearing price is the `max_price` of the **marginal bid**, the last bid to receive any units, which may be filled only in part. Bids ranked after it receive nothing.
3. **No bids.** There is no clearing price (`None`), and the whole supply goes back to the seller.

**Tie-breaking.** Bids at the same price are filled in submission order. With 500 units and three 300-unit bids at 20, the first gets 300, the second 200, and the third nothing. A later bid at a *higher* price still outranks an earlier, lower one.

**Worked example.** With 1,000 units, a reserve of 10, and bids A 400@30, B 500@25, C 300@20, D 200@15:

| Bid | Rank | Escrow | Allocation | Pays (×20) | Refund |
|-----|-----:|-------:|-----------:|-----------:|-------:|
| A 400@30 | 1 | 12,000 | 400 | 8,000 | 4,000 |
| B 500@25 | 2 | 12,500 | 500 | 10,000 | 2,500 |
| C 300@20 | 3 | 6,000 | 100 (marginal) | 2,000 | 4,000 |
| D 200@15 | 4 | 3,000 | 0 | 0 | 3,000 |

Cumulative demand reaches 1,000 inside C, so C is the marginal bid and the clearing price is 20. The seller receives 20,000.

**Why no winner overpays.** Every filled bid ranks at or above the marginal bid, so `clearing_price ≤ max_price` for every winner. Allocations never exceed quantities, so `payment = allocation × clearing ≤ quantity × max_price = escrow` and no refund is negative. The contract also checks this and refuses with `EscrowMismatch` rather than over-charging.

## Settlement and refunds

Batch payouts are **pull-based**:

- `settle(id)` (after `end_time`, once) works out the clearing price, sends the seller `units_sold × clearing_price` and any unsold units, and records the result. It reads the order book but writes only the auction entry.
- `claim(id, bidder)` (after settlement, once per bid) works out that bid's fill again from the ranked book (`min(quantity, supply − units ranked ahead)`). It then sends the allocation in asset units and the refund `escrow − allocation × clearing_price` in payment tokens, and marks the bid `claimed`. Anyone may call it, because the funds can only go to the bidder.

The accounting holds as an invariant. After settlement, `total_escrow` equals the sum of all refunds still owed. Each claim reduces it by exactly that bid's refund, so once every bid has been claimed the contract holds zero of both tokens for the auction.

<div class="tk-callout">
  <span class="tk-callout__title">Why not pay everyone from <code>settle</code>?</span>
  <p>An earlier version of this example did exactly that. Measuring it against the SDK's Mainnet invocation limits showed the problem. Each payout is a token transfer, and each transfer emits an event. At 50 bidders, settlement produced about 17.9 KB of contract events, more than the 16 KiB a Mainnet transaction may carry. Pull-based claims keep <code>settle</code> nearly constant in cost and every <code>claim</code> bounded, and <code>tests/budget_storage.rs</code> checks a full 64-bid book against both the Testnet and Mainnet limits.</p>
</div>

### Error conditions

| Error (code) | Cause |
|--------------|-------|
| `AuctionNotFound` (101) | Unknown auction id |
| `InvalidConfig` (102) | A schedule, supply, reserve or window that cannot work (see above) |
| `NotStarted` (103) | Bid before `start_time` |
| `BiddingClosed` (104) | Dutch bid after `start_time + duration`; batch bid at or after `end_time` |
| `BiddingOpen` (105) | Settle before the window closes (a Dutch lot that has already sold can be settled at once) |
| `AlreadySettled` (106) | Settle, or bid, after settlement |
| `BidBelowPrice` (107) | Dutch maximum below the asking price; batch price below the reserve |
| `InvalidQuantity` (108) | Batch quantity ≤ 0 |
| `DuplicateBid` (109) | A second batch bid from the same bidder |
| `LotAlreadySold` (110) | A Dutch bid after the lot was taken |
| `TooManyBids` (111) | More than `MAX_BATCH_BIDS` bids |
| `ArithmeticOverflow` (112) | An escrow, demand total, or price calculation would overflow |
| `WrongAuctionKind` (113) | A Dutch call on a batch auction, or the reverse |
| `SellerCannotBid` (114) | The seller bidding on their own auction |
| `EscrowMismatch` (115) | Settlement accounting would not return every escrowed token. Never raised by a correct build |
| `BidNotFound` (116) | `get_bid` or `claim` for an address that did not bid |
| `NotSettled` (117) | `claim` before settlement |
| `AlreadyClaimed` (118) | A second `claim` for the same bid |

## The test suite

### The `House` fixture (`tests/common/mod.rs`)

`House::new(n)` builds a `TestContext` with `TestContextBuilder::new().with_users(n)` (`House::with_profile` adds a `NetworkProfile`). It sets the clock, registers the contract and two Stellar Asset Contracts, mints the seller 1,000,000 asset units, and gives each bidder 1,000,000,000 payment tokens. Helpers keep the tests short: `dutch()`, `dutch_with(start, floor, duration, step)`, `batch()`, `batch_with(supply, reserve)`, `place_bids(id, &[(qty, price), …])`, `close_window()`, `claim_all(id, n)`, `advance(secs)` (which calls `TestContext::advance_time`), and balance readers.

### 1. Fixtures (`tests/fixtures.rs`)

The full lifecycle, with exact numbers:

- **Dutch.** Escrow on creation. Linear and stepped decay. Rounding. A bid refused at 10,000 and accepted 200 s later at 8,000. The winner pays the asking price, not their maximum. A bid at the last second pays the floor, and one second later bidding is refused. Settlement delivers the lot, and an unsold lot goes back to the seller.
- **Batch.** Escrow per bid. Undersubscribed clears at the reserve. Oversubscribed clears at the marginal bid (the table above). Ties fill in submission order, and a later higher bid outranks an earlier one. Demand equal to supply clears at the reserve. Settlement and claims are run with 1, 3, 10 and 25 bidders. With no bids, the supply comes back to the seller.
- **Insufficient balance.** A bid the bidder cannot cover fails and leaves no trace (Dutch and batch).

### 2. Assertions (`tests/assert.rs`)

- `assert_contract_error!` pins the exact error for every rejection above: invalid configs, an overflowing schedule, unknown ids, wrong kind, a bid below the price, the lot already sold, out-of-window bids, duplicate bids, bad quantities, an overflowing escrow, the seller bidding, the book cap, settlement and claims at the wrong time or repeated.
- `assert_host_error!` checks that an under-funded bid surfaces the token's `Error(Contract, #10)`.
- `EventMatcher` reads the `batch_bid`, `dutch_bid`, `claimed` and `settled` payloads, including the clearing price.
- `assert_auth_matches!` confirms that bids need the bidder's signature and creation needs the seller's.
- `assert_balance_conserved!` shows that settlement and claims conserve both tokens across the contract, the seller and every bidder.

### 3. Budget and storage (`tests/budget_storage.rs`)

**Benchmark: 5, 20 and 50 bidders.** The scenario is fixed so runs are comparable. Bidder `i` asks for 10 units at `10 + i`, and supply is `5 × bidders`, so the auction is exactly 2× oversubscribed. The test measures each invocation with `BudgetSnapshot::last_invocation` and reports:

- the single `settle` call;
- the sum of all `claim` calls;
- the most expensive single claim;
- the sum of all bids.

It asserts the clearing price, a `BudgetGuard` ceiling of 100M instructions / 40 MiB for `settle` and for the worst claim, and that costs grow with bidder count. These are the numbers measured for this PR (`cargo test -p soroban-testkit-example-auction --test budget_storage -- --nocapture`):

```text
bidders | settle cpu | settle mem | all claims cpu | all claims mem | worst claim cpu | worst claim mem | all bids cpu | all bids mem
      5 |     311333 |      58675 |        2266140 |         423819 |          527607 |           98329 |      1854181 |       384612
     20 |     417835 |      98575 |       11294312 |        2555823 |          703495 |          164613 |     10248708 |      2640297
     50 |     611710 |     178375 |       40453546 |       11321808 |         1002916 |          293441 |     38407643 |     12132492
```

These are host-metered costs for a **native** test contract. VM instantiation and Wasm execution are not included, so a deployed Wasm build costs more. The figures are deterministic for a given `Cargo.lock`. The per-claim cost rises with the bidder count because each claim scans the ranked book up to its own position.

**Baseline.** `budget.json` commits `settle_{5,20,50}_bidders` and `claims_{5,20,50}_bidders`. `settlement_costs_stay_within_the_committed_baseline` fails if any of them grows more than 10%. To re-record after an intended change, run the ignored recorder:

```bash
cargo test -p soroban-testkit-example-auction --test budget_storage -- --ignored budget_baseline_records_current_costs
```

**Network limits.** `a_full_order_book_fits_the_network_resource_limits` settles and claims a full 64-bid book twice, once under `NetworkProfile::Testnet` and once under `NetworkProfile::Mainnet`. With a profile installed, the host enforces that network's per-invocation limits, so any call that broke one would fail. The test also compares the measured `cost_estimate().resources()` of `settle` and of the worst-case claim with the profile's `invocation_limits()`, covering instructions, memory, read and write entries, footprint, bytes and event size. If a check fails, it names the resource that went over.

**Storage.** `StorageSnapshot` shows:

- creation adds `[Auction, 0]` (persistent) and `[NextId]` (instance), and nothing temporary;
- each bid adds exactly one persistent `[Bid, 0, …]` entry and rewrites `[Book, 0]` and `[Auction, 0]`;
- settlement modifies only `[Auction, 0]`;
- a claim modifies only the auction and that bidder's record.

### 4. Properties (`tests/proptest.rs`)

The bid books are random: 1–12 bids, quantities 1–200, prices from the reserve to 200, and supply 1–1,000. Dutch schedules draw their start time from `soroban_testkit_generators::strategies::timestamp()` and their prices from `token_amount()`.

| Property | Invariant |
|----------|-----------|
| `a_winning_bidder_never_pays_more_than_their_maximum_bid` | Every filled bid has `clearing ≤ max_price`, and what each bidder lost equals their recorded payment, which is at most `allocation × max_price` |
| `allocations_never_exceed_supply_and_follow_the_documented_rule` | Total allocation = `min(supply, demand)` ≤ supply, and the clearing price and every allocation match an independent model of the documented rule |
| `escrow_accounting_conserves_both_tokens` | `payment + refund = escrow` for every bid, proceeds = `units_sold × clearing`, both token totals are unchanged, and the contract ends holding zero |
| `with_distinct_prices_submission_order_does_not_matter` | Rotating the submission order of distinct-price bids changes neither the clearing price nor anyone's allocation |
| `dutch_price_is_non_increasing_and_bounded` | The asking price never rises, stays within [floor, start], matches the published formula, and ends at the floor |
| `dutch_bid_is_accepted_exactly_when_it_covers_the_asking_price` | A Dutch bid succeeds if and only if `max_price ≥ asking`, and then pays exactly `asking` |

## Running it

```bash
cargo test -p soroban-testkit-example-auction
```

```bash
cargo test -p soroban-testkit-example-auction --test budget_storage -- --nocapture
```

## Assumptions and limitations

- **Escrow.** A Dutch bidder escrows the asking price at the moment they bid. A batch bidder escrows their full `quantity × max_price` until they claim. Funds are never pulled from a bidder at settlement.
- **One bid per bidder** in a batch auction. Bids cannot be amended or withdrawn.
- **Allocation is by price, then time priority.** There is no pro-rata split at the clearing price, so the earlier of two tied bids is filled first.
- **Undersubscribed batch auctions clear at the reserve.** Without competition for the supply, every bidder pays the seller's minimum rather than the lowest bid.
- **The book is capped at 64 bids** so that settlement and the worst-case claim stay well inside a transaction's limits.
- **Unclaimed refunds stay in escrow** until someone calls `claim`. There is no expiry or sweep.
- **Cost figures are native-contract measurements.** They are not network fees. With soroban-sdk 28, a Wasm build needs `stellar contract build` (stellar-cli v25.2.0+); a plain `cargo build --target wasm32v1-none` is refused by the SDK's build script.
