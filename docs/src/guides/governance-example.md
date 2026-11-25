# Governance DAO Worked Example

This guide walks through `examples/governance`, a multi-party DAO with weighted
voting, a quorum and a timelock. It exercises all four testkit crates: fixtures
for members and the ledger clock, assertions for multi-user auth, events and
typed errors, core snapshots for storage tiers, storage diffs and budget, and
generators for property tests over generated voter sets.

## What the DAO Does

Members hold non-transferable **voting power**. The admin mints it at launch;
afterwards only a passed proposal can mint more.

1. **Propose** — any member with voting power opens a proposal carrying an
   `Action`. Voting opens immediately and closes `voting_period` seconds
   later. The total supply is snapshotted.
2. **Vote** — each member votes once, for or against, weighted by their power.
   The weight is written to a per-voter receipt.
3. **Queue** — once voting has closed, anyone may call `queue`. The proposal
   passes when turnout (for + against) reaches the quorum — `quorum_bps` of the
   supply snapshot, rounded up — **and** the votes for strictly exceed the votes
   against. A passed proposal is `Queued` with `eta = now + timelock`; otherwise
   it is `Defeated`.
4. **Execute** — anyone may execute a queued proposal once the ledger clock
   reaches `eta`. Execution applies the action and marks it `Executed`.

The proposer may cancel while voting is open.

```text
propose ──► Pending ──(voting period ends)──► queue ─┬─► Queued ──(timelock)──► execute ──► Executed
              │                                      └─► Defeated
              └─(proposer)──► cancel ──► Cancelled
```

```rust
use governance::{Action, Governance, GovernanceClient};

let id = env.register(Governance, ());
let dao = GovernanceClient::new(&env, &id);

dao.initialize(&admin, &4_000, &86_400, &172_800); // 40% quorum, 1-day vote, 2-day timelock
dao.mint(&alice, &450);
dao.mint(&bob, &350);
dao.mint(&carol, &200);

let proposal = dao.propose(&alice, &Symbol::new(&env, "raise_quorum"), &Action::SetQuorum(6_000));
dao.vote(&alice, &proposal, &true);
dao.vote(&bob, &proposal, &true);

ctx.advance_time(86_400);   // voting closes
dao.queue(&proposal);       // 800 of 1 000 voted, 800 for → Queued
ctx.advance_time(172_800);  // timelock elapses
dao.execute(&proposal);     // quorum is now 60%
```

## Contract Surface

| Function | Auth | Purpose |
|----------|------|---------|
| `initialize(admin, quorum_bps, voting_period, timelock)` | admin | Validates and stores the configuration |
| `mint(to, amount)` | admin | Adds voting power and total supply |
| `propose(proposer, title, action) -> u64` | proposer | Opens a `Pending` proposal; the proposer needs voting power |
| `vote(voter, id, support) -> i128` | voter | Casts a weighted vote once; returns the weight counted |
| `queue(id) -> ProposalStatus` | none | Decides a closed vote: `Queued` or `Defeated` |
| `execute(id)` | none | Applies a `Queued` proposal at or after its `eta` |
| `cancel(proposer, id)` | proposer | Withdraws a `Pending` proposal |
| `proposal`, `voting_power`, `total_supply`, `quorum_bps`, `voting_period`, `timelock`, `proposal_count`, `receipt`, `quorum_votes`, `admin` | none | Read-only getters |

`Action` is one of `SetQuorum(u32)`, `SetVotingPeriod(u64)`, `SetTimelock(u64)`
or `Mint(Address, i128)`. Actions are validated at proposal time, so a proposal
can never pass and then fail to execute on bad parameters.

Failures are typed `GovernanceError` values (`VotingOpen`, `VotingClosed`,
`AlreadyVoted`, `NotQueued`, `TimelockNotElapsed`, …), and each state change
emits one contract event (`proposal_created`, `vote_cast`, `proposal_queued`,
`proposal_defeated`, `proposal_executed`, `proposal_cancelled`).

### Storage layout

| Tier | Keys |
|------|------|
| Instance | `Admin`, `QuorumBps`, `VotingPeriod`, `Timelock`, `TotalSupply`, `NextProposalId` |
| Persistent | `Power(Address)`, `Proposal(u64)`, `Receipt(u64, Address)` |

Configuration is small and read on every call, so it shares the contract
instance entry. Per-member and per-proposal records grow without bound, so each
gets its own persistent entry with an extended TTL.

## The Test Suite

53 tests across four files plus a shared fixture in `tests/common/mod.rs`.

### The fixture

`Fixture` builds a `TestContext` with `TestContextBuilder::with_users(3)`,
starts the clock at a fixed timestamp, and registers the contract. Constructors
stack: `Fixture::new()` (registered), `initialized()`, and `funded()` (voters
hold 450 / 350 / 200 = 1 000 votes, so the 40% quorum is 400). Helpers drive
the clock through the fixtures crate's `TestContext::advance_time`:

```rust
let mut fixture = Fixture::funded();
let id = fixture.queued(&Action::SetQuorum(6_000)); // propose, two votes, end_voting, queue
fixture.advance_time(TIMELOCK);
fixture.client().execute(&id);
```

### `tests/fixtures.rs` — life cycle (25 tests)

Configuration validation, minting, proposal windows and ids, weighted tallies,
double-vote and no-power rejection, the voting deadline, and every way a vote
is decided: below quorum, majority against, a tie, and a pass followed by
execution of `SetQuorum` and `Mint` actions. One test runs the whole life cycle
under `NetworkProfile::Testnet` so it is metered against testnet limits.

### `tests/assert.rs` — auth, events and timelock bypass (14 tests)

Multi-user authorization with `AuthMatcher`: each vote requires exactly the
voter's auth, propose needs the proposer, mint needs the admin, and `queue` and
`execute` need no auth at all (`assert_no_auth_required`). A vote whose
signature belongs to a different member is refused and leaves the tally
untouched.

Timelock bypass attempts, each checked with `assert_contract_error!`:

| Attempt | Error |
|---------|-------|
| Execute a proposal that is still `Pending` | `NotQueued` |
| Execute one second before `eta` | `TimelockNotElapsed` |
| Execute twice | `NotQueued` |
| Execute a `Defeated` proposal long after | `NotQueued` |
| Re-queue a `Queued` proposal to push its `eta` around | `NotPending` |

`execution_unlocks_exactly_at_the_eta` pins the boundary from the other side.
`EventMatcher::with_topic` checks that each transition emits exactly one event
of the right kind.

### `tests/budget_storage.rs` — storage tiers, diffs and budget (8 tests)

`assert_storage_tier` confirms every configuration key lives in instance
storage and that `[Power*`, `[Proposal*` and `[Receipt*` keys live in
persistent storage, with nothing temporary.

`StorageSnapshot::diff` follows a proposal through its life cycle:

```rust
let pending = StorageSnapshot::capture(env, &dao);
dao.queue(&id);
let queued = StorageSnapshot::capture(env, &dao);

let change = pending.diff(&queued);
let proposal = change.modified().iter().find(|c| c.key == "[Proposal, 0]").unwrap();
assert!(proposal.before.contains("Pending"));
assert!(proposal.after.contains("Queued"));
```

The same pattern checks `Queued → Executed` (and that execution rewrote
`[QuorumBps]`), that a vote adds exactly one receipt and modifies only the
proposal, and that a rejected execution leaves storage byte-for-byte
unchanged. `BudgetSnapshot` shows a vote consumes budget and that vote and
queue fit inside the testnet CPU and memory limits.

### `tests/proptest.rs` — generated voters (6 properties)

Members are generated with `strategies::account_address()` and converted to
host addresses with `AddressBytes::to_address`, each with a random weight and a
random choice (for, against, abstain).

- **Quorum across generated voters** — for 1–8 members, `queue` returns exactly
  what a reference model computes from the weights, and `quorum_votes` matches
  the rounded-up basis-point formula.
- **Vote weight conservation** — `votes_for + votes_against` equals the summed
  weight of everyone who voted, equals the sum of their receipts, and never
  exceeds the supply snapshot.
- **Repeated votes** never change the tally.
- **Timelock enforcement** — for every elapsed time in `0..TIMELOCK`, execution
  fails and the configuration is unchanged; for any time at or past it,
  execution succeeds.
- **Voting deadline** — every vote at or after `end` is refused.

## Running It

```bash
cargo test -p governance
cargo clippy -p governance --all-targets -- -D warnings
```
