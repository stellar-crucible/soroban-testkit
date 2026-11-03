# Worked Example: Token

A second contract and its suite, written to show the parts [`Worked Example: Counter`](./counter-example.md) cannot: an event payload with three fields in it, two contracts registered in one environment, an allowance one holder grants another, and generated inputs. Everything here is real code in [`examples/token`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/token), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/token</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">37 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-token</span></div>
</div>

## The contract

An ERC-20 shaped token: a supply that only moves through `mint`, `burn` and
`transfer`, balances in persistent storage, allowances a spender draws down, and
one event per state change carrying the values that changed.

```rust
#[contractevent(topics = ["token", "transfer"])]
#[derive(Clone)]
pub struct Transfer {
    pub from: Address,
    pub to: Address,
    pub amount: i128,
}

pub fn transfer(env: Env, from: Address, to: Address, amount: i128) {
    from.require_auth();
    move_balance(&env, &from, &to, amount);
    Transfer { from, to, amount }.publish(&env);
}
```

The three fields are the reason this example exists. The counter's event carries
one number, so its suite asserts on that number and nothing else; a token test
has to say which of `from`, `to` and `amount` it means.

## Why four files

The package splits its suite by crate, one file each, so a reader can open the
file for the crate they are learning and find nothing else in it.

| File | Crate | What it covers |
| --- | --- | --- |
| `src/test.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, `user(index)`, `add_user`, the ledger clock, `reset` |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `EventData`, `EventLog`, `assert_auth_matches!`, `AuthMatcher` |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | `budget_guard!`, `BudgetSnapshot`, `BudgetBaseline`, `StorageSnapshot`, `assert_storage_tier!` |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` over `token_amount()`, `account_address()`, `contract_address()` |

`src/test.rs` is a module of the `no_std` crate, so it reaches the contract
directly with `use crate::{TokenContract, TokenContractClient}`. The three files
under `tests/` are separate binaries and name the package instead:

```rust
use soroban_testkit_example_token::{TokenContract, TokenContractClient};
```

## Fixtures — the environment

```rust
fn context(users: usize) -> TestContext {
    TestContextBuilder::new().with_users(users).build()
}

fn token(ctx: &TestContext) -> (Address, TokenContractClient<'_>) {
    let contract_id = ctx.env.register(TokenContract, ());
    let client = TokenContractClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin);
    (contract_id, client)
}
```

Two things the counter never needed. A token has holders, so the builder is
asked for them up front and `ctx.user(index)` is the accessor — with a panic
message that names the `with_users` call to fix when the index is out of range.
And a token has an admin: `initialize` records `ctx.admin`, which is what makes
the fixture's admin a contract concept rather than a test variable.

`reset()` is the other fixture idea worth a test of its own:

```rust
ctx.reset();
let (_id, client) = token(&ctx);

assert_eq!(client.balance_of(&holder), 0);
assert_eq!(client.total_supply(), 0);
```

The same holders, a brand-new chain. Registration belongs to the environment
that created it, so the contract is re-registered and every byte of its storage
is gone — which is exactly the second-phase state a multi-phase test wants
without tearing the whole fixture down.

## Assert — events, then authorisations

`EventMatcher` reads the data payload as a map keyed by field name, so a test
asserts the value that moved:

```rust
EventMatcher::new(&ctx.env)
    .from_contract(&contract_id)
    .with_topic("transfer")
    .assert_data_matches(|data| {
        data.field("amount")
            .and_then(|value| value.deserialize::<i128>())
            == Some(250)
    });
```

Three behaviours of that API the counter's suite has no room to show.

**A field that has no value is absent, not void.** The SDK publishes event maps
sparse, so `Minted` — which has no `from` — reads `data.field("from").is_none()`,
while `Transfer` reads `Some`. `fields()` enumerates what is really in the map,
which pins an event's shape as well as its values.

**`from_contract` matters once two contracts share an environment.** The token
example registers two of them in one test and asserts that one matcher answers
while the other stays silent. Nothing about the fixture changed between them.

**The scope is the most recent invocation.** This is the trap in both suites, and
the example walks into it on purpose in prose rather than in code: reading state
back after the assertion is one more invocation, and the invocation the matcher
was looking at is gone with it. So the event assertions come first and the
`balance_of` checks after. `EventLog` is how a test opts out of that scope — it
keeps every call it collects, so a sequence of `mint`, `transfer` and `approve`
can be judged as one set:

```rust
let mut log = EventLog::new(&ctx.env);
client.mint(sender, &1_000);
log.collect();
client.transfer(sender, recipient, &250);
log.collect();
client.approve(sender, spender, &100);
log.collect();

log.matcher().with_topic("transfer").assert_count(1);
assert_eq!(log.len(), 3);
```

Auth works the same way. `assert_auth_matches!` reads `env.auths()` for the
latest invocation, and under the builder's default policy the requirement is
recorded rather than enforced:

```rust
assert_auth_matches!(ctx.env, {
    invoker: sender,
    function: "transfer",
    args: [_, recipient, 250i128],
});
```

`AuthMatcher` answers the questions the macro does not ask — `assert_auth_count(1)`
after a `transfer`, `assert_no_auth_required()` after a `balance_of` — and one
test drops the mocked policy entirely and supplies the `MockAuth` itself, which
is how you prove the contract really asked.

## Core — cost and storage tier

`budget_and_storage.rs` builds a bare `Env` instead of a `TestContext`, so the
file contains no other crate.

```rust
soroban_testkit_core::budget_guard!(
    &env,
    "mint",
    { cpu_max: 20_000_000, mem_max: 20_000_000 },
    || client(&env, &contract).mint(&holder, &1_000)
);
```

A ceiling is transaction headroom, not the measured cost — a contract registered
natively is metered without the VM's own costs. Crossing one fails with one
parseable line, which a `#[should_panic(expected = "...")]` test pins:

```text
BUDGET kind=ceiling case=mint metric=cpu_insns actual=… limit=1
```

`BudgetSnapshot::last_invocation` is per call, so a read and a mint in the same
test are two readings, and `diff` is the gap between them. `BudgetBaseline`
turns a reading into a number CI can commit; one test round-trips a baseline
through `to_json_string` and `parse` to show the recorded cases survive it.

The storage half is what a token has and a counter does not: three tiers in one
contract.

```rust
soroban_testkit_core::assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);
soroban_testkit_core::assert_storage_tier!(&env, &contract, "balances", StorageTier::Persistent);

let snapshot = StorageSnapshot::capture(&env, &contract);
assert_eq!(snapshot.in_tier(StorageTier::Persistent).len(), 2);
```

The pattern argument takes `prefix*` and `*suffix` as well as an exact key, so
`"*es"` asserts the pair `balances` and `allowances` in one line. A key in the
tier nobody meant is a failure with the key, the expected tier and the found one
in the message, and `StorageDiff` says which entry a call added — the first
`mint` adds `persistent:balances` and rewrites `instance:supply`, which is a
storage read nobody intended made visible.

## Generators — properties over the whole contract

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn a_transfer_moves_exactly_what_it_reports(
        held in token_amount(),
        moved in token_amount(),
        sender in account_address(),
        recipient in contract_address(),
    ) {
        prop_assume!(moved <= held);

        let env = Env::default();
        let client = deploy(&env);
        let sender = sender.to_address(&env);
        let recipient = recipient.to_address(&env);

        client.mint(&sender, &held);
        client.transfer(&sender, &recipient, &moved);

        prop_assert_eq!(client.balance_of(&recipient), moved);
        prop_assert_eq!(client.balance_of(&sender), held - moved);
        prop_assert_eq!(client.total_supply(), held);
    }
}
```

Each case makes its own `Env`, so no case inherits another's storage, and
`prop_assume!` is how a case asks for a relation between two generated values —
here that the transfer fits inside the balance. The allowance test asks for two:
the spend has to fit the allowance *and* the balance.

The recipient is generated with `contract_address()` and the sender with
`account_address()`, which is the point of the two strategies: both formats are
opaque 32 bytes on the wire, so a contract that silently assumed a `G...` account
fails here rather than on mainnet. Two more cases judge the strategies
themselves — that generated addresses never collide, and that the requested
format is the XDR shape that arrives.

<div class="tk-callout">
  <span class="tk-callout__title">Sixty-four cases, not two hundred and fifty-six</span>
  <p>Every case registers a contract and runs several calls on the host. The file keeps its cases low enough that <code>cargo test --workspace</code> stays quick; the strategy-only block in the same file runs 256, because a case over plain numbers costs nothing.</p>
</div>

## Running it

```bash
cargo test -p soroban-testkit-example-token
```

```text
test test::initialize_records_the_admin_and_starts_the_supply_at_zero ... ok
test test::the_builder_hands_out_as_many_distinct_holders_as_the_test_asks_for ... ok
test test::asking_for_a_holder_that_was_never_built_points_at_with_users - should panic ... ok
test test::reset_keeps_the_holders_and_empties_the_chain ... ok
                                        # src/test.rs — 7 tests, fixtures

test a_transfer_event_carries_the_amount_and_both_parties_in_its_data ... ok
test a_mint_payload_omits_the_field_that_has_no_value ... ok
test two_tokens_in_one_environment_keep_their_events_apart ... ok
test the_matcher_counts_one_calls_requirement_and_a_reads_none ... ok
                                        # tests/assert_events.rs — 11 tests

test mint_stays_inside_its_transaction_ceiling ... ok
test a_call_past_its_ceiling_fails_the_test_with_a_parseable_line - should panic ... ok
test initialize_writes_only_instance_storage ... ok
test every_key_a_token_holds_reads_back_qualified ... ok
                                        # tests/budget_and_storage.rs — 11 tests

test a_transfer_moves_exactly_what_it_reports ... ok
test an_allowance_is_drawn_down_by_what_was_spent ... ok
test generated_addresses_never_collide ... ok
                                        # tests/property.rs — 8 tests
```

37 tests in the package, and all of them inside `cargo test --workspace --locked`.

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Property testing</h3>
    <p class="tk-card__body"><a href="./property-testing.html">Strategies, cases and the per-run nonce</a> behind <code>tests/property.rs</code>.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Worked Example: Counter</h3>
    <p class="tk-card__body"><a href="./counter-example.html">The smaller example</a> this one builds on, and the budget baseline file.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">soroban-testkit-core</h3>
    <p class="tk-card__body"><a href="../crates/core.html">StorageSnapshot and BudgetBaseline</a>, the two APIs <code>tests/budget_and_storage.rs</code> uses.</p>
  </div>
</div>
