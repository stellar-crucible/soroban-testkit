# Worked Example: Bounty Escrow

A task bounty with a multi-signature release: a creator locks funds, a
contributor submits work, named reviewers approve, and the escrow pays out only
when enough of them have signed — or refunds the creator once the deadline has
passed without a release. Everything here is real code in
[`examples/bounty_escrow`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/bounty_escrow),
a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/bounty_escrow</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">48 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-bounty-escrow</span></div>
</div>

## The contract

One [`#![no_std]`](https://doc.rust-lang.org/reference/attributes.html) contract
holds every bounty in a persistent map and the next id in instance storage. A
bounty moves through four states, and exactly one of the last two is reachable:

```text
Open ──submit──▶ Submitted ──threshold approvals──▶ Released
  │                   │
  └──────refund───────┴─────(deadline passed)────▶ Refunded
```

```rust
#[contracttype]
pub enum BountyStatus {
    Open,
    Submitted,
    Released,
    Refunded,
}

pub fn create(
    env: Env,
    creator: Address,
    token: Address,
    amount: i128,
    reviewers: Vec<Address>,
    threshold: u32,
    deadline: u64,
) -> u64 {
    creator.require_auth();
    assert!(amount > 0, "amount must be positive");
    assert!(!reviewers.is_empty(), "at least one reviewer is required");
    assert!(
        threshold > 0 && threshold <= reviewers.len(),
        "threshold must be between 1 and the reviewer count"
    );

    // ... store the bounty, then move the creator's tokens into the contract
    token::Client::new(&env, &token).transfer(
        &creator,
        &env.current_contract_address(),
        &amount,
    );
    id
}
```

The escrow is the contract's own token balance. `create` moves the money in, so
a bounty that is never claimed still has funds the creator can reclaim; `approve`
moves it out on the signature that reaches the threshold; `refund` moves it back
after the deadline. `submit` and `approve` both reject a clock at or past the
deadline, which leaves refund as the only way the funds can move once the window
closes.

## Why four files

The package splits its suite by crate, one file each, so a reader can open the
file for the crate they are learning and find nothing else in it.

| File | Crate | What it covers |
| --- | --- | --- |
| `src/test.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, `user(index)`, `add_user`, the ledger clock, `reset`, and the contract's own validation rules |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `EventData`, `EventLog`, `assert_auth_matches!`, `AuthMatcher` |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | `budget_guard!`, `BudgetSnapshot`, `BudgetBaseline`, `StorageSnapshot`, `assert_storage_tier!`, and the payout benchmark |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` over `token_amount()`, `contract_address()`, and `host_error_code_coverage()` |

`src/test.rs` is a module of the `no_std` crate, so it reaches the contract
directly with `use crate::{BountyEscrowContract, BountyEscrowContractClient}`.
The three files under `tests/` are separate binaries and name the package
instead:

```rust
use soroban_testkit_example_bounty_escrow::{
    BountyEscrowContract, BountyEscrowContractClient, BountyStatus,
};
```

## Fixtures — three roles and a clock

```rust
fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

fn deploy(ctx: &TestContext, funding: i128) -> (Address, Address, BountyEscrowContractClient<'_>) {
    let contract_id = ctx.env.register(BountyEscrowContract, ());
    let client = BountyEscrowContractClient::new(&ctx.env, &contract_id);
    let token = ctx
        .env
        .register_stellar_asset_contract_v2(ctx.admin.clone())
        .address();
    StellarAssetClient::new(&ctx.env, &token).mint(ctx.user(0), &funding);
    (contract_id, token, client)
}
```

The suite assigns the fixture's identities to roles rather than inventing them
per test: index 0 is the creator and the funded holder, index 1 is the
contributor, and every index from 2 is a reviewer. `reviewers(ctx, 3)` builds
that list, which is the value `create` takes.

The clock is a fixture concept too. `ctx.timestamp()` is what `open` adds a day
to, and `ctx.env.ledger().set_timestamp(...)` is how a test moves past the
deadline without waiting:

```rust
let deadline = ctx.timestamp() + 86_400;
// ... later, in the refund test
ctx.env.ledger().set_timestamp(deadline);
```

`reset()` clears the chain without clearing the identities, so the multi-phase
test re-registers the contract and proves the next bounty starts from id 1 with
an empty balance. `add_user` grows the context after the fact, which is the
reviewer that arrives late.

## Assert — five event kinds, then the authorisations

Every transition publishes an event, and the payload is what the assertions read
as a map keyed by field name:

```rust
EventMatcher::new(&ctx.env)
    .from_contract(&contract_id)
    .with_topic("created")
    .assert_data_matches(|data| {
        data.field("amount")
            .and_then(|value| value.deserialize::<i128>())
            == Some(1_000)
    });
```

The suite asserts five shapes: `bounty/created` with its terms, `bounty/submitted`
with the contributor, `bounty/approved` with the running count and the threshold,
`bounty/released` with the amount paid, and `bounty/refunded` with the amount
returned. Two of those are the interesting ones.

**The approval event carries its progress.** `approvals` and `threshold` travel
in the data, so a test proves the count grew rather than only that a signature
was recorded:

```rust
for expected in 1..=2u32 {
    client.approve(&id, &ctx.user(1 + expected as usize));
    EventMatcher::new(&ctx.env)
        .from_contract(&contract_id)
        .with_topic("approved")
        .assert_data_matches(|data| {
            data.field("approvals")
                .and_then(|value| value.deserialize::<u32>())
                == Some(expected)
        });
}
```

**`no_release_before_the_threshold_is_reached`** is the negative half: after one
of two signatures the `released` topic is silent, and the balance has not moved.
Together with the positive release test it pins the multi-signature rule from
both sides.

Auth is the other half of this file. `assert_auth_matches!` names the invoker,
function and arguments of the latest invocation:

```rust
assert_auth_matches!(ctx.env, {
    invoker: contributor,
    function: "submit",
    args: [id, contributor],
});
```

`AuthMatcher` answers what the macro does not ask: `assert_auth_count(1)` after
`create`, `submit`, `approve` and `refund`, and `assert_no_auth_required()` after
a read. The file also proves the refusals: an address that is not a reviewer
cannot approve, and a reviewer cannot sign twice.

## Core — ceilings, a payout benchmark, and two storage tiers

`budget_and_storage.rs` builds a bare `Env` instead of a `TestContext`, so the
file contains no other testkit crate.

```rust
soroban_testkit_core::budget_guard!(&fixture.env, "create", {
    cpu_max: 50_000_000,
    mem_max: 50_000_000,
}, || fixture.open(1_000));
```

A ceiling is transaction headroom, not a measured cost. Crossing one fails with
one parseable line, which a `#[should_panic(expected = "...")]` test pins:

```text
BUDGET kind=ceiling case=create metric=cpu_insns actual=… limit=1
```

**The payout benchmark is the reason this file is more than a guard.** A release
costs whatever the signatures before it cost, so `payout_cost(reviewer_count)`
sums `BudgetSnapshot::last_invocation` across the approvals:

```rust
fn payout_cost(reviewer_count: usize) -> BudgetSnapshot {
    let fixture = setup(reviewer_count);
    let client = fixture.client();
    let id = fixture.open(1_000);
    client.submit(&id, &fixture.contributor);

    let mut total = BudgetSnapshot { cpu_insns: 0, mem_bytes: 0 };
    for reviewer in fixture.reviewers.iter() {
        client.approve(&id, &reviewer);
        let step = BudgetSnapshot::last_invocation(&fixture.env);
        total.cpu_insns += step.cpu_insns;
        total.mem_bytes += step.mem_bytes;
    }
    total
}
```

`payout_cost_grows_with_the_reviewer_count` measures 1, 2, 3, 5 and 7 reviewers,
prints the table for a reviewer to read, and asserts the shape rather than a
machine-specific number: each larger quorum costs more instructions, and seven
signatures write more memory than one. `a_baseline_records_one_payout_case_per_reviewer_count`
turns those readings into `BudgetBaseline` cases (`payout/1`, `payout/3`,
`payout/5`) and round-trips them through `to_json_string` and `parse`, which is
what lets a pull request compare its costs against a file recorded on `main`.

The storage half shows the two tiers the contract uses:

```rust
soroban_testkit_core::assert_storage_tier!(
    &fixture.env, &fixture.contract, "next_id", StorageTier::Instance
);
soroban_testkit_core::assert_storage_tier!(
    &fixture.env, &fixture.contract, "bounties", StorageTier::Persistent
);
```

`StorageDiff` says what a transition really wrote. The first `create` adds both
keys; a later `approve` modifies `persistent:bounties` and leaves
`instance:next_id` alone; a refund rewrites exactly one entry; and a set of reads
leaves the snapshot unchanged. The deliberately wrong tier test pins the failure
message:

```text
Storage tier mismatch for key 'next_id': expected persistent, found instance
```

## Generators — the threshold, the clock, and host errors

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn funds_are_released_only_at_the_reviewer_threshold(
        threshold in 1u32..=4u32,
        signatures in 0u32..=5u32,
    ) {
        let fixture = setup(1_000, 4);
        let client = fixture.client();
        let id = fixture.open(500, threshold);
        client.submit(&id, &fixture.contributor);

        let applied = signatures.min(threshold);
        for index in 0..applied {
            fixture.approve(id, index);
        }

        let bounty = client.get(&id);
        if signatures >= threshold {
            prop_assert_eq!(bounty.status, BountyStatus::Released);
            prop_assert_eq!(fixture.paid_out(), 500);
        } else {
            prop_assert_eq!(bounty.status, BountyStatus::Submitted);
            prop_assert_eq!(fixture.escrow(), 500);
        }
    }
}
```

That is the acceptance criterion in code: across every generated threshold and
signature count, the escrow pays the contributor **only** once the threshold is
reached, and otherwise holds the full amount. `any_threshold_inside_the_reviewer_set_always_releases`
complements it over generated reviewer sets from 1 to 6, so the rule is proven
for every legal threshold rather than only the fixed one the unit tests use.

Amounts come from `token_amount()`, which can generate zero; the escrow
properties `prop_assume!(amount > 0)` because a zero bounty is refused at
creation, a different property from the one each case is about. Wherever the
money sits, `the_escrow_holds_exactly_the_bounty` asserts the invariant:
`escrow + paid_out == amount`, and the creator's balance is zero.

The clock is generated too: `a_submission_after_the_deadline_is_refused` and
`an_approval_after_the_deadline_is_refused` add `0..=10_000` seconds past the
deadline — zero included, so the exact boundary is covered — and assert the call
fails while the escrow stays put. `a_settled_bounty_refuses_further_moves` proves
a released bounty cannot be paid twice: a late signature and a refund both fail.

**The host-error case is the new generator in action.** `host_error_code_coverage()`
draws from all ten `ScErrorType` categories and all ten `ScErrorCode` values, and
the suite fuzzes an error-recovery block over the whole domain:

```rust
enum Recovery {
    Retry,
    Stop,
}

fn recover(error: &Error) -> Recovery {
    if error.is_type(ScErrorType::Budget) || error.is_type(ScErrorType::Storage) {
        Recovery::Retry
    } else {
        Recovery::Stop
    }
}

#[test]
fn every_generated_host_error_is_recoverable_around_a_rejected_action(
    error in host_error_code_coverage(),
) {
    // ... a non-reviewer's approval is refused and the state is intact
    let decision = recover(&error.as_error());
    // every category gets a decision; only transient ones ask for a retry
}
```

Each case also proves the rejected action left storage untouched, so a contract
that handled an error by half-writing state would fail here. See
[crate docs for generators](../crates/generators.md) for the strategy's tables,
`HostError` accessors, and the shrink-to-`contract/0` behaviour.

<div class="tk-callout">
  <span class="tk-callout__title">Forty-eight cases, each with its own environment</span>
  <p>Every property case registers two contracts — the escrow and a Stellar asset contract — and runs several calls on the host. The file keeps its case count at 48 so <code>cargo test --workspace</code> stays quick; each case builds its own <code>Env</code>, so no case inherits another's storage or balances.</p>
</div>

## Running it

```bash
cargo test -p soroban-testkit-example-bounty-escrow
```

```text
test test::creating_a_bounty_locks_the_creators_funds_in_the_contract ... ok
test test::the_fixture_clock_is_what_the_deadline_is_measured_against ... ok
test test::reset_keeps_the_identities_and_empties_the_chain ... ok
                                        # src/test.rs — 13 tests, fixtures
                                        #   plus the contract's validation rules

test a_created_event_carries_the_bounty_terms ... ok
test every_approval_reports_the_running_count ... ok
test no_release_before_the_threshold_is_reached ... ok
test a_reviewer_cannot_approve_twice ... ok
                                        # tests/assert_events.rs — 14 tests

test a_multi_signature_release_stays_inside_its_ceiling ... ok
test payout_cost_grows_with_the_reviewer_count ... ok
test a_baseline_records_one_payout_case_per_reviewer_count ... ok
test a_key_in_the_wrong_tier_is_a_failure - should panic ... ok
                                        # tests/budget_and_storage.rs — 12 tests

test funds_are_released_only_at_the_reviewer_threshold ... ok
test any_threshold_inside_the_reviewer_set_always_releases ... ok
test every_generated_host_error_is_recoverable_around_a_rejected_action ... ok
                                        # tests/property.rs — 9 tests
```

48 tests in the package, and all of them inside `cargo test --workspace --locked`.
The payout benchmark prints its table with `--nocapture`:

```bash
cargo test -p soroban-testkit-example-bounty-escrow payout_cost_grows_with_the_reviewer_count -- --nocapture
```

```text
reviewers= 1 cpu_insns=… mem_bytes=…
reviewers= 2 cpu_insns=… mem_bytes=…
reviewers= 3 cpu_insns=… mem_bytes=…
reviewers= 5 cpu_insns=… mem_bytes=…
reviewers= 7 cpu_insns=… mem_bytes=…
```

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Property testing</h3>
    <p class="tk-card__body"><a href="./property-testing.html">Strategies, cases and the per-run nonce</a> behind <code>tests/property.rs</code>.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Budget-Aware Testing</h3>
    <p class="tk-card__body"><a href="./budget-testing.html">Ceilings, baselines and the parseable failure line</a> this suite's guards use.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">soroban-testkit-generators</h3>
    <p class="tk-card__body"><a href="../crates/generators.html"><code>host_error_code_coverage()</code></a> and every other strategy this package draws from.</p>
  </div>
</div>
