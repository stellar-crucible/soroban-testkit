# Subscription Billing Worked Example

This guide walks through `examples/subscription`, a recurring payment contract
that draws each cycle from the subscriber's token allowance. Its suite
demonstrates all four Testkit crates together.

## The Problem

A merchant bills the same customer the same amount on a schedule. On Soroban
that needs three separate guarantees:

1. The subscriber, not the merchant, chooses the fee and the interval.
2. No cycle pays out before its time, and no cycle pays twice.
3. The subscriber can stop future payments without the merchant's permission.

A single transfer does not model this: the merchant would have to be trusted to
ask for the right amount at the right moment. The example keeps the terms on
chain and lets the contract be the only party that can move the funds.

## Protocol Design

The subscriber creates a subscription and approves the subscription contract as
a spender on a SEP-41 token. The allowance is the hard, token-level ceiling on
what the contract can ever pull. On top of it, the contract enforces its own
terms: one stored fee, one interval, and one `next_billing` timestamp.

A billing call then does exactly four things:

1. Checks that the caller is the recorded merchant.
2. Checks that the subscription is not canceled.
3. Checks that the ledger timestamp has reached `next_billing`.
4. Transfers the stored fee from subscriber to merchant, then advances
   `next_billing` by one interval.

The merchant never passes an amount. There is no setter for the fee or the
interval, so the merchant cannot widen the agreement after the subscriber has
authorized it.

## Contract Architecture

The whole record lives under one persistent key:

```rust
#[contracttype]
pub enum DataKey {
    Subscription(Address),
}

#[contracttype]
pub struct Subscription {
    pub subscriber: Address,
    pub merchant: Address,
    pub token: Address,
    pub fee: i128,
    pub interval: u64,
    pub next_billing: u64,
    pub cycles_billed: u32,
    pub canceled: bool,
}
```

One subscriber holds at most one live subscription. A canceled record can be
replaced when the subscriber wants new terms, which keeps the state model small
enough to read in one storage entry.

```rust
use soroban_testkit_example_subscription::{SubscriptionContract, SubscriptionContractClient};

let contract_id = env.register(SubscriptionContract, ());
let client = SubscriptionContractClient::new(&env, &contract_id);

client.subscribe(&subscriber, &merchant, &token, &500, &86_400, &start + 86_400);
```

`subscribe` validates the fee (positive), the interval (nonzero) and the first
billing time (not in the past), stores the record and publishes a
`SubscriptionCreated` event. Failures are typed `SubscriptionError` values, so a
test asserts the cause rather than a panic string.

| Function | Purpose |
|----------|---------|
| `subscribe` | Records the terms; requires the subscriber's authorization |
| `bill` | Charges one cycle; merchant-only, due-only |
| `cancel` | Stops future charges; subscriber-only |
| `subscription` | Reads the stored record |
| `is_active` | Whether a live subscription exists |
| `is_billing_due` | Whether the next cycle is payable now |

## Subscriber Authorization

Creating a subscription is an authorized action:

```rust
subscriber.require_auth();
```

The authorization covers the terms, because they are arguments to the call that
recorded them. The tests read the requirement back with the assert crate rather
than only proving the call succeeded:

```rust
assert_auth_matches!(fixture.ctx.env, {
    invoker: subscriber,
    function: "subscribe",
    args: [_, merchant, token, 500i128, 86_400u64, next_billing],
});
```

Billing requires the merchant's authorization, and the contract then compares
the authorizing address with the merchant stored at creation. An outsider can
authorize a call all day; it still returns `Unauthorized` and moves nothing.
Cancellation requires the subscriber, so the merchant cannot lock a customer
into a schedule.

## Recurring Allowance Deductions

The allowance is approved on the token, from the subscriber to the subscription
contract. The merchant is never a spender and never touches the allowance
directly.

```rust
token::Client::new(&env, &token).transfer_from(
    &env.current_contract_address(),
    &subscriber,
    &subscription.merchant,
    &subscription.fee,
);
```

Every successful cycle therefore deducts exactly `fee` from the allowance and
credits exactly `fee` to the merchant. The test suite asserts both sides of that
movement, and `assert_balance_conserved!` proves the cycle created and destroyed
no tokens:

```rust
assert_balance_conserved!(
    accounts: [(&subscriber, "subscriber"), (&merchant, "merchant")],
    balance: |address| fixture.balance(address),
    call: {
        fixture.bill();
    }
);
```

Two failure modes are covered explicitly:

- An allowance below the fee makes the token refuse the transfer and the
  contract returns `PaymentFailed` without changing the schedule.
- A balance below the fee fails the same way, because an allowance is a
  permission rather than a source of funds.

Because the contract charges the stored fee and takes no amount argument, the
allowance can only ever be drawn in fee-sized steps.

## Billing Period Checks

`next_billing` is the first timestamp at which the cycle is payable. Billing
compares it with `env.ledger().timestamp()`:

```rust
if env.ledger().timestamp() < subscription.next_billing {
    return Err(SubscriptionError::BillingNotDue);
}
```

That gives the boundary behaviour the suite pins:

- One second early is rejected with `BillingNotDue`.
- The exact boundary second is accepted.
- A second call straight after a successful one is rejected; the schedule has
  already moved.
- After a long gap, each call still charges one cycle and advances the schedule
  by one interval. A merchant catching up makes one call per elapsed period,
  never a single call that drains several periods at once.

`next_billing` advances by `interval` rather than by "now", so the schedule does
not drift with the moment billing happened. The addition is checked:

```rust
let next_billing = subscription
    .next_billing
    .checked_add(subscription.interval)
    .ok_or(SubscriptionError::TimestampOverflow)?;
```

At the end of the `u64` clock the call returns `TimestampOverflow` before any
transfer, and the stored schedule stays where it was.

## Simulating Recurring Billing with `advance_time`

The fixture wraps `TestContext`, so the suite moves the real ledger clock through
the fixtures crate:

```rust
let mut fixture = Fixture::new();
fixture.subscribe();

fixture.advance_time(INTERVAL);
assert_eq!(fixture.bill(), 1);

fixture.advance_time(INTERVAL);
assert_eq!(fixture.bill(), 2);
```

`advance_time` returns the new timestamp and leaves the ledger sequence alone,
which is what subscription tests want: the schedule is measured in seconds, not
in ledger closes. The fixture starts its clock at a fixed timestamp so the
expected `next_billing` values are absolute and readable.

## Cancellation

Cancellation is one authorized write:

```rust
pub fn cancel(env: Env, caller: Address, subscriber: Address) -> Result<(), SubscriptionError> {
    caller.require_auth();
    // caller must be the stored subscriber, and the record must not already be canceled
    subscription.canceled = true;
    env.storage().persistent().set(&key, &subscription);
    Ok(())
}
```

After that, `bill` returns `AlreadyCanceled` forever, `is_billing_due` is false,
and the merchant's balance stops changing. The tests also cover canceling twice,
canceling an unknown subscriber, and a merchant or outsider attempting the call.

## Test Layout

The suite is split across four files under `examples/subscription/tests`, with
more than 30 tests:

| File | Focus |
|------|-------|
| `fixtures.rs` | Creation, validation, schedule getters and cancellation state |
| `assert.rs` | Authorization, events, exact fee movement, every rejection path and the timestamp overflow |
| `budget_storage.rs` | CPU and memory measurements, storage tiers, diffs and key namespace auditing |
| `proptest.rs` | Generated terms proving the per-cycle withdrawal limit |

A shared `tests/common/mod.rs` builds a `Fixture` containing a `TestContext`, a
deployed Stellar asset contract, a funded subscriber, a merchant, an outsider
and the subscription contract. The subscriber is funded and the contract is
approved in the fixture, so each test starts from a usable agreement.

### Fixtures

```rust
use soroban_testkit_fixtures::builder::TestContextBuilder;

let mut ctx = TestContextBuilder::new().with_users(3).build();
ctx.set_timestamp(START);
```

`TestContext` supplies the environment, users and clock. `advance_time` comes
from the same crate, so a billing test never hand-builds a `LedgerInfo`.

### Assertions

The assert crate covers three questions in this example:

- Who authorized the call (`assert_auth_matches!`, `AuthMatcher`).
- What the contract published (`EventMatcher`, including event payload fields).
- Whether a cycle conserved token balances (`assert_balance_conserved!`).

The storage-quota matcher from the same crate checks that a subscription adds
exactly one persistent entry.

### Budget and Storage

The core crate supplies the measurements:

```rust
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};

let cycles = BudgetGuard::new("bill")
    .cpu_ceiling(5_000_000)
    .mem_ceiling(1_000_000)
    .run(&fixture.ctx.env, || fixture.bill());
```

`BudgetSnapshot::last_invocation` also shows that a read of the schedule costs
less than a billing cycle. `StorageSnapshot` proves that subscribe adds one
persistent entry and that billing and cancellation rewrite it instead of growing
storage, and `NamespaceAuditor` checks the `DataKey::Subscription` namespace.

## The Property Test

The central property is:

> A merchant can never withdraw more than the subscriber-authorized recurring fee
> per billing cycle.

Each generated case builds a funded subscription with a generated fee, advances
the clock by a generated amount, and makes several billing attempts. After every
call it checks that:

- a successful cycle moved exactly `fee` to the merchant;
- a rejected cycle moved nothing;
- total withdrawals equal `fee × cycles_billed`;
- total withdrawals never exceed `fee × (elapsed / interval)`.

Other properties repeat calls inside one cycle (all must fail), advance time and
verify one charge per interval, cancel and verify no withdrawal, bill from an
unauthorized address, charge at generated timestamp boundaries, and run the same
flow with a generated account address as subscriber and a generated contract
address as merchant. Fees come from
`soroban_testkit_generators::strategies::token_amount`-style ranges, timestamps
from `timestamp()`, and the address formats from `account_address()` and
`contract_address()`.

```rust
proptest! {
    #[test]
    fn a_merchant_never_withdraws_more_than_the_fee_per_cycle(
        fee in recurring_fee(),
        elapsed in 0u64..=(INTERVAL * 3),
        attempts in 1usize..=6,
    ) {
        // ... bill `attempts` times and assert every delta is either fee or zero
    }
}
```

## Running the Example

From the repository root:

```bash
cargo test -p soroban-testkit-example-subscription
cargo clippy -p soroban-testkit-example-subscription --all-targets -- -D warnings
```

The example is a workspace member, so the top-level commands exercise it too:

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Security Assumptions and Limitations

- **The allowance is the outer ceiling.** The contract enforces one `fee` per
  cycle, but it can only pull what the subscriber has approved on the token.
  Approving an amount smaller than the fee makes billing fail with no state
  change; approving a larger amount does not allow a larger single charge
  because the amount is never caller-supplied.
- **The token is trusted.** A malicious token could re-enter `bill`. The
  contract advances the schedule before calling the token and restores the old
  record if the transfer fails, so a re-entrant call on the same cycle sees
  `BillingNotDue`. A token that reports success without moving funds is outside
  this example's threat model.
- **One subscription per subscriber.** The key is the subscriber address, so a
  subscriber cannot hold two live agreements at once. Multi-merchant
  subscriptions would key by `(subscriber, merchant)`.
- **Catch-up is per call.** A long gap does not produce a single multi-cycle
  charge; the merchant calls once per elapsed period. If the subscriber cancels
  first, the gap is never collected.
- **No pause, refund or proration.** Cycles are charged at the stored interval
  and fee. A partial period is not refunded, and billing is not paused for
  anyone but the subscriber.
- **Persistent TTL is not extended.** The subscription entry is written to
  persistent storage but the contract does not bump its TTL, so a production
  deployment must extend it or the record can expire. The example keeps the
  storage story about which tier is used, not about rent management.
- **No events beyond the three lifecycle events.** `SubscriptionCreated`,
  `Billed` and `SubscriptionCanceled` are the only observable outputs; a
  successful cycle is not reported anywhere else.
