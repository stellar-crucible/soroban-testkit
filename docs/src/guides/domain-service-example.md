# Worked Example: Domain Service

A third contract and its suite, written to show what the counter and token examples cannot: a registry whose validity is a function of the ledger clock, records that resolve only while a term lasts, a grace period a second user can wait out, and storage that grows per registration instead of per key. Everything here is real code in [`examples/domain_service`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/domain_service), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/domain_service</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">39 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-domain-service</span></div>
</div>

## The contract

A name registry: a name is registered for a term of whole years, resolved through three record kinds (a Stellar address, an IPFS content id, free text), transferable while its term runs, renewable through a grace period, and claimable by anyone once that grace period has passed. Fees are paid from an internal credit ledger, so the example needs no token contract to demonstrate a fee path.

```rust
pub fn resolve(env: Env, name: String, kind: RecordKind) -> Option<String> {
    let domain = domains(&env).get(name.clone())?;
    if domain.expires_at <= env.ledger().timestamp() {
        return None;
    }
    records(&env).get(name)?.get(kind)
}
```

That body is the whole reason the example exists. Validity is read from `env.ledger().timestamp()`, not from a stored flag, so `TestContext::advance_time` is what expires a name — and resolution is a property of the term rather than of the key, which is why an expired name stops resolving even though its record is still in storage.

## Why four files

The package splits its suite by crate, one file each, so a reader can open the file for the crate they are learning and find nothing else in it.

| File | Crate | What it covers |
| --- | --- | --- |
| `src/test.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, `user(index)`, `add_user`, `advance_time` across a term, `reset` |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `EventData`, `EventLog`, `assert_auth_matches!`, `AuthMatcher` |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | `budget_guard!`, `BudgetSnapshot`, `BudgetBaseline`, `StorageSnapshot`, `assert_storage_tier!` |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` over names, terms, addresses and the claim-after-grace invariant |

`src/test.rs` is a module of the `no_std` crate, so it reaches the contract directly with `use crate::{DomainService, DomainServiceClient}`. The three files under `tests/` are separate binaries and name the package instead:

```rust
use soroban_testkit_example_domain_service::{
    DomainService, DomainServiceClient, RecordKind, SECONDS_PER_YEAR,
};
```

## Fixtures — a clock, not a flag

```rust
fn registry(ctx: &TestContext) -> (Address, DomainServiceClient<'_>) {
    let contract_id = ctx.env.register(DomainService, ());
    let client = DomainServiceClient::new(&ctx.env, &contract_id);
    client.initialize(&ctx.admin, &ctx.admin, &100, &604_800);
    for user in ctx.users.iter() {
        client.deposit(user, &1_000);
    }
    (contract_id, client)
}
```

The fixture's admin is both operator and treasury, so a test does not have to carry a second identity around to check a fee. Every user is given credits up front, which is what makes the renewal and claim tests about the *term* rather than about funding.

The clock is the point of this example. A term is expired by moving the ledger, and the difference between "expired" and "claimable" is the grace period:

```rust
let mut ctx = context(1);
ctx.set_timestamp(1_700_000_000);
let env = ctx.env.clone();
let (id, _) = registry(&ctx);
let client = DomainServiceClient::new(&env, &id);
let label = name(&ctx, "alice");
client.register(ctx.user(0), &label, &1);

ctx.advance_time(SECONDS_PER_YEAR - 86_400);
assert!(!client.is_available(&label));

ctx.advance_time(86_400 + 604_800 + 1);
assert!(client.is_available(&label));
```

`advance_time` takes `&mut self` and the client borrows the context's env, so the test clones the env handle before it starts: the client reads the same ledger the fixture moves.

<div class="tk-callout">
  <span class="tk-callout__title">Registration belongs to the env</span>
  <p><code>reset()</code> swaps in a brand-new chain and keeps every identity, so a registry test has to re-register and re-initialize the contract afterwards. That is also what makes <code>reset</code> a two-phase test's tool rather than a teardown: the same holders, a clean ledger.</p>
</div>

## Assert — events and authorisations

A registration publishes three fields, and each is asserted on its own:

```rust
EventMatcher::new(&ctx.env)
    .from_contract(&contract_id)
    .with_topic("registered")
    .assert_data_matches(|data| {
        data.field("expires_at")
            .and_then(|value| value.deserialize::<u64>())
            == Some(1_700_000_000 + SECONDS_PER_YEAR)
    });
```

Three behaviours the smaller examples have no room to show.

**A registry can be registered twice in one environment.** The suite builds two registries and asserts that the one that published answers while the other stays silent, which is only possible because `from_contract` filters on the emitter.

**A record event carries a typed field.** `RecordKind` is a `contracttype` enum, so `data.field("kind").and_then(|field| field.deserialize::<RecordKind>())` reads it back as the value the contract published rather than as a number.

**A sequence of calls can be judged as one set.** `EventLog` keeps `registered`, `record` and `transferred` across three invocations, and each topic is counted over the sequence:

```rust
log.matcher().with_topic("registered").assert_count(1);
log.matcher().with_topic("record").assert_count(1);
log.matcher().with_topic("transferred").assert_count(1);
assert_eq!(log.len(), 3);
```

Auth works the same way. `assert_auth_matches!` reads `env.auths()` for the latest invocation, and the name argument is left as `_` because an equality on a `String` would need the env-converted handle rather than the Rust one:

```rust
assert_auth_matches!(ctx.env, {
    invoker: owner,
    function: "register",
    args: [_, _, 3u32],
});
```

`AuthMatcher` answers the questions the macro does not ask — `assert_auth_count(1)` after a registration, `assert_no_auth_required()` after `owner_of` — and one test drops the mocked policy entirely and proves the contract really asked.

## Core — cost, tier, and growth per name

`budget_and_storage.rs` builds a bare `Env` instead of a `TestContext`, so the file contains no other crate.

```rust
soroban_testkit_core::budget_guard!(
    &env,
    "register",
    { cpu_max: 20_000_000, mem_max: 20_000_000 },
    || client(&env, &contract).register(&owner, &name(&env, "alice"), &1)
);
```

A ceiling is transaction headroom, not the measured cost — a contract registered natively is metered without the VM's own costs. Crossing one fails with one parseable line, which a `#[should_panic(expected = "BUDGET kind=ceiling case=register metric=cpu_insns")]` test pins.

The storage half is what this contract has and the token does not: **growth inside one entry**.

```rust
client.register(&first, &name(&env, "alice"), &1);
let after_first = StorageSnapshot::capture(&env, &contract);
client.register(&second, &name(&env, "bob"), &1);
let after_second = StorageSnapshot::capture(&env, &contract);

assert!(after_second.get("domains").unwrap().value.len() > after_first.get("domains").unwrap().value.len());
assert_eq!(after_first.entries().len(), after_second.entries().len());
```

Every name lives in one persistent `domains` map, so a registration rewrites that entry instead of opening a new key. That is the shape the storage benchmark measures: bytes per name, not keys per name. `StorageSnapshot` reports the rendered value, so the test can compare its length before and after without a second copy of the map.

The tiers are the other half:

```rust
assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);
assert_storage_tier!(&env, &contract, "domains", StorageTier::Persistent);
assert_storage_tier!(&env, &contract, "records", StorageTier::Persistent);
```

Configuration lives in instance storage, registry state in persistent storage, and the wildcard `"*s"` form asserts the pair `domains` and `records` in one line. `StorageDiff` says which entry a call added — the first registration adds `persistent:domains` — and a deposit adds only `persistent:balances`, which is how a test proves the fee path did not touch the registry.

## Generators — the claim-after-grace invariant

```rust
proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn an_expired_name_can_be_claimed_by_a_new_owner_after_the_grace_period(
        years in 1u32..=5,
        suffix in "[a-z]{2,6}",
    ) {
        // ... register for `years`, fund a second user
        let expires_at = client.expires_at(&label).expect("a registered name has an expiry");
        env.ledger().set_timestamp(expires_at + GRACE + 1);

        prop_assert!(client.is_available(&label));
        let claimed = client.claim(&claimant, &label);
        prop_assert_eq!(claimed.owner, claimant.clone());
        prop_assert_eq!(claimed.expires_at, env.ledger().timestamp() + SECONDS_PER_YEAR);
    }
}
```

The property is the one the grace period exists for: **every** term length, and every name the generator can produce, becomes claimable exactly one second past expiry plus grace — and is not claimable before it. A second property covers the other side, walking the clock to an arbitrary second inside the grace period and asserting the previous owner still holds the name.

Two more judge the name rules against the contract itself. `is_valid_name` is a view, so a property can ask it without paying for a registration; the acceptance test then registers every generated valid name, which catches a validator that disagrees with the writer. The refusal test generates uppercase starts, two-character names, trailing punctuation and leading hyphens, and asserts the view says no to all of them.

The address strategies matter here for the transfer property: the owner is generated with `account_address()` and the recipient with `contract_address()`, so a registry that silently assumed a `G...` account fails in the suite rather than on mainnet.

## Running it

```bash
cargo test -p soroban-testkit-example-domain-service
```

```text
test test::initialize_records_the_operator_the_treasury_and_the_terms ... ok
test test::advancing_the_clock_is_what_expires_a_name ... ok
test test::a_name_inside_its_grace_period_is_still_unavailable ... ok
test test::reset_keeps_the_holders_and_empties_the_registry ... ok
                                        # src/test.rs — 10 tests, fixtures

test a_registration_event_carries_the_name_the_owner_and_the_expiry ... ok
test a_record_event_carries_the_kind_and_the_value_it_stored ... ok
test a_sequence_of_calls_can_be_asserted_as_one_set ... ok
test a_registration_records_the_owners_authorization_with_its_arguments ... ok
                                        # tests/assert_events.rs — 10 tests

test a_registration_stays_inside_its_transaction_ceiling ... ok
test every_registration_grows_the_one_registry_entry ... ok
test a_key_in_the_wrong_tier_is_a_failure - should panic ... ok
                                        # tests/budget_and_storage.rs — 10 tests

test an_expired_name_can_be_claimed_by_a_new_owner_after_the_grace_period ... ok
test every_generated_valid_name_is_accepted ... ok
test records_stop_resolving_when_the_term_expires ... ok
                                        # tests/property.rs — 9 tests
```

39 tests in the package, and all of them inside `cargo test --workspace --locked`.

## Where to go next

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Worked Example: Token</h3>
    <p class="tk-card__body"><a href="./token-example.html">The transfer-and-allowance example</a> this one borrows its four-file shape from.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Budget-Aware Testing</h3>
    <p class="tk-card__body"><a href="./budget-testing.html">Ceilings, baselines and the committed file</a> behind the fee-path guards.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Crate</span>
    <h3 class="tk-card__title">soroban-testkit-generators</h3>
    <p class="tk-card__body"><a href="../crates/generators.html">Names, terms and ledger values</a> as strategies, plus the XDR object fuzzer.</p>
  </div>
</div>
