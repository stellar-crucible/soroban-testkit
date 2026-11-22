# Worked Example: Staking and Yield Farming

`examples/staking` is a staking pool with a reward stream funded separately from
the principal: stakers deposit a token, the distributor pays a fixed number of
reward stroops per second for the whole pool, and each staker's share accrues in
proportion to their stake. It exists to show the two things a staking contract
gets wrong — the reward accumulator and the exit — under test.

The contract keeps one accumulator, reward stroops per staked stroop scaled by
`SCALE` (`1e12`), and each position remembers the accumulator value it was last
paid against. A claim is therefore

```text
amount * (pool.reward_per_token - position.reward_per_token_paid) / SCALE
```

which is why every `stake`, `unstake` and `claim` settles the accumulator before
it touches a position: a deposit that skipped that step would retroactively earn
the rewards that accrued while it was not in the pool.

## What the contract does

| Call | Behaviour |
|------|-----------|
| `initialize(admin, stake_token, reward_token, reward_rate)` | One pool, one admin, no second initialization |
| `stake(owner, amount)` | Settles, transfers in, records the position, restarts the lock |
| `unstake(owner, amount)` | Settles, pays the principal (minus a penalty inside the lock) |
| `claim(owner)` | Pays the settled reward and resets it |
| `emergency_withdraw(owner)` | Paused-only full exit; forfeits unsettled rewards |
| `set_reward_rate(rate)` | Settles at the old rate, then changes it |
| `set_paused(bool)` | Refuses new stakes; never blocks exits |
| `earned(owner)` | A view: settled rewards plus what has accrued since |

- Rewards accrue from the ledger timestamp, so `advance_time` in the fixture
  moves the clock the contract reads.
- A withdrawal before `MIN_LOCK_SECONDS` (seven days) pays
  `EARLY_EXIT_PENALTY_BPS` (10%) of the withdrawn amount to the admin, taken from
  principal.
- An empty pool advances its timestamp without paying anybody, so an idle window
  is not handed to the next staker.
- Rewards live in the contract, minted by the distributor: a claim larger than
  the balance is a configuration bug and the transfer fails rather than minting.

## The five test files

| File | What it covers |
|------|----------------|
| `tests/common/mod.rs` | The fixture: two funded stakers, a funded reward pool, `advance_time` |
| `tests/fixtures.rs` | Deposits, top-ups, partial and full exits, penalties, claims, reward splits, rate changes |
| `tests/assert.rs` | The explicit `MockAuth` tree, refusals, paused behaviour, emergency exit, the idle window |
| `tests/budget_storage.rs` | Budget ceilings for `stake` and `claim`, a view measuring less than a write, instance vs persistent storage, storage diffs, a baseline round trip |
| `tests/proptest.rs` | Principal survival, penalty exactness, reward conservation across two stakers, monotonic views |

```bash
cargo test -p soroban-testkit-example-staking
```

## The properties that matter

```rust
proptest! {
    #[test]
    fn two_stakers_share_the_stream_without_creating_value(
        alice in 1i128..=1_000 * ONE,
        bob in 1i128..=1_000 * ONE,
        seconds in 1u64..=1_000,
    ) {
        let fixture = Fixture::new();
        fixture.client().stake(&fixture.alice, &alice);
        fixture.client().stake(&fixture.bob, &bob);
        fixture.advance_time(seconds);

        let total = fixture.client().earned(&fixture.alice)
            + fixture.client().earned(&fixture.bob);
        let stream = 1_000 * seconds as i128;

        prop_assert!(total <= stream);
        prop_assert!(stream - total <= 2); // one stroop lost per floor division
    }
}
```

The `<= 2` is the point of the example: two integer divisions round down, so the
pool can keep at most one stroop per staker, and a test that allowed more would
hide a real leak. The same style of property pins the early exit
(`returned + penalty == amount`) and a full round trip (`staked == 0`,
`total_staked == 0`, principal returned exactly).

## Storage and cost

The pool lives in one instance entry and each staker in one persistent entry, so
a first stake adds exactly one key and a top-up rewrites two values without
adding any:

```rust
let diff = before.diff(&after);
assert_eq!(diff.added().len(), 0);
assert_eq!(diff.modified().len(), 2);
```

`budget_guard!` holds `stake` and `claim` under a transaction ceiling, and a view
is measured against a write to show that `earned` costs less than the `claim`
that pays the same number — the difference is the transfer and the two storage
writes, not the arithmetic.
