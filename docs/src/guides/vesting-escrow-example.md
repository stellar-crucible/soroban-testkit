# Worked Example: Vesting Escrow

A token vesting escrow with a cliff and linear unlock: an admin locks a grant, a
beneficiary claims vested tokens over time, and the admin may revoke the
unvested portion at any moment. Everything here is real code in
[`examples/vesting_escrow`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/vesting_escrow),
a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/vesting_escrow</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">31 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-vesting-escrow</span></div>
</div>

## The contract

One `#![no_std]` contract holds every schedule in a persistent map and the next
id in instance storage. A schedule is `Active` until it is either fully claimed
(`Completed`) or revoked (`Revoked`):

```text
Active ──claim──▶ Active (until fully claimed) ──▶ Completed
  │
  └──revoke──▶ Revoked (vested remainder stays claimable)
```

```rust
#[contracttype]
pub enum ScheduleStatus { Active, Revoked, Completed }

pub fn create_schedule(
    env: Env, admin: Address, beneficiary: Address, token: Address,
    total_amount: i128, start: u64, cliff_duration: u64, duration: u64,
    revocable: bool,
) -> u64;
pub fn claim(env: Env, id: u64) -> i128;
pub fn revoke(env: Env, id: u64) -> i128;
```

Vesting is zero until the cliff, then linear over the full duration:

```rust
fn vested_at(schedule: &VestingSchedule, now: u64) -> i128 {
    if now < schedule.start { return 0; }
    let elapsed = now - schedule.start;
    if elapsed < schedule.cliff_duration { return 0; }
    if elapsed >= schedule.duration { return schedule.total_amount; }
    schedule.total_amount * elapsed as i128 / schedule.duration as i128
}
```

Revocation freezes the vested cap at the revocation timestamp and returns only
the unvested portion to the admin; the beneficiary keeps claiming what had
already vested.

## Tests

- `tests/fixtures.rs` — 10 tests: creation locks tokens, sequential ids, cliff
  boundary, linear quarters, claim payout, accrual, revocation freeze, and
  non-revocable / fully-vested rejection.
- `tests/assert_events.rs` — 8 tests: event payloads, `EventLog` ordering,
  and authorization matching for `create_schedule`, `claim`, and `revoke`.
- `tests/budget_and_storage.rs` — 10 tests: ceilings, tier placement, and the
  committed `budget.json` baseline.
- `tests/property.rs` — 3 proptests: the conservation invariant
  `claimed + claimable + unvested + returned == total`, monotonic vesting, and
  claims never exceeding the vested amount.
