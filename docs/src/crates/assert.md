# soroban-testkit-assert

Fluent matchers for contract events and authorizations, so assertions read like the behaviour you expect.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-assert</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">events · invocation · auth · balance · storage · budget_diff · budget</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Style</span><span class="tk-spec__value">chainable builder</span></div>
</div>

## API at a glance

| Method | Asserts |
|--------|---------|
| `EventMatcher::assert_emitted()` | At least one matching event exists |
| `EventMatcher::assert_not_emitted()` | No matching event exists |
| `EventMatcher::assert_none_match(pred)` | No matching event satisfies a predicate |
| `EventMatcher::assert_count(n)` | Exactly `n` events were emitted |
| `EventMatcher::assert_count_within(bound)` | The number of matching events falls within a range (or equals a count) |
| `EventMatcher::assert_topic_count(topic, bound)` | The number of events carrying `topic` falls within a range (or equals a count) |
| `assert_event_count_range!(env, 3..=5)` | Macro form of `assert_count_within`, over an `Env` or an `EventLog` |
| `assert_event_topic_count!(env, "Transfer", 2)` | Macro form of `assert_topic_count`, over an `Env` or an `EventLog` |
| `EventMatcher::assert_data_matches(pred)` | One event's payload satisfies a predicate |
| `EventMatcher::from_contract(addr)` | Restricts matches to one contract address |
| `EventMatcher::with_topic(t)` | Restricts matches to events carrying that topic symbol |
| `EventLog::collect()` | Gathers one invocation's events into a set spanning calls |
| `EventLog::matcher()` | A matcher over everything that log collected |
| `assert_event_data_schema!(env, topic, { field: Type, .. })` | Checks matching event maps for required fields, exact keys, and deserializable types |
| `assert_no_contract_error!(client.try_call(..))` | Unwraps a successful client call or reports its decoded Soroban error |
| `assert_call_success_with_value!(client.try_call(..), expected)` | Checks decoded call success and return-value equality |
| `assert_contract_error!(client.try_call(..), MyError::Variant)` | The call failed with exactly that `#[contracterror]` variant |
| `assert_host_error!(client.try_call(..), ScErrorType::Auth)` | The call failed with a host error of that category, category and code, or exact `Error` |
| `AuthMatcher::assert_no_auth_required()` | The call needed no authorizations |
| `AuthMatcher::assert_auth_count(n)` | Exactly `n` authorizations were recorded |
| `AuthMatcher::assert_matches()` | One recorded authorization fits the configured invoker, function, and arguments |
| `assert_auth_matches!(env, { .. })` | The same, with the invoker, function, and argument pattern inline |
| `AuthSubTreeMatcher::assert_matches()` | A matching authorization sub-tree appears at any depth in recorded auths |
| `assert_auth_sub_tree_matches!(env, { .. })` | Inline sub-tree matcher with argument and child wildcards and side-by-side failure trees |
| `AuthSubInvocationMatcher::assert_matches()` | An authorized sub-call matching contract, function, and arguments appears at any depth |
| `assert_auth_sub_invocation!(env, { .. })` | Inline sub-invocation matcher isolating intermediate contract calls at any depth |
| `StorageQuotaMatcher::assert_matches()` | Asserts storage tier counts satisfy exact quotas or upper bounds |
| `assert_storage_tier_count!(env, contract, { .. })` | Macro enforcing allocation quotas across instance, persistent, and temporary tiers |
| `assert_balance_conserved!(..)` | Asserts total token balance conservation across accounts, fee pools, and reserves |
| `assert_budget_diff_within!(..)` | Asserts relative gas savings between baseline and optimized closures with side-by-side failure diff table |

## Event matching

Replace raw tuple iteration with a fluent API:

```rust
use soroban_testkit_assert::events::EventMatcher;

// Assert at least one event was emitted
EventMatcher::new(&env).assert_emitted();

// Assert exact count
EventMatcher::new(&env).assert_count(3);

// Filter by contract and topic
EventMatcher::new(&env)
    .from_contract(&contract_id)
    .with_topic("Transfer")
    .assert_emitted();
```

<div class="tk-callout">
  <span class="tk-callout__title">Scope: the invocation that just ran</span>
  <p><code>EventMatcher::new(&amp;env)</code> reads <code>env.events().all()</code>, and in SDK v28 that returns the events of the <strong>most recent contract invocation</strong>. Assert right after each call, as the examples here do. To judge several calls as one set, collect them into an <a href="#asserting-across-several-invocations"><code>EventLog</code></a> — same filters, same assertions, wider scope.</p>
</div>

<div class="tk-callout">
  <span class="tk-callout__title">Reading events in v28</span>
  <p><code>env.events().all()</code> now returns <code>ContractEvents</code>. Call <code>.events()</code> on it to get the slice, and import <code>soroban_sdk::testutils::Events as _</code> — the method is feature-gated behind <code>testutils</code>. Testkit applies the contract and topic filters for you.</p>
</div>

## Asserting across several invocations

A multi-step test usually wants a sentence about a whole sequence: "these three calls emitted exactly two `Transfer` events, and nothing was ever refunded". `EventLog` gathers the events of each call the test means to judge, and hands the set to the same matcher:

```rust
use soroban_testkit_assert::events::{EventLog, EventMatcher};

let mut log = EventLog::new(&env);

client.transfer(&from, &to, &100);
log.collect();
client.close_offer(&from);
log.collect();

// Every filter and assertion reads the sequence as one set.
log.matcher().assert_count(2);
log.matcher().with_topic("Transfer").assert_count(1);
log.matcher().from_contract(&contract_id).assert_emitted();
log.matcher().with_topic("Refund").assert_not_emitted();

// The latest call alone still works, unchanged.
EventMatcher::new(&env).with_topic("CloseOffer").assert_count(1);
```

Collection is explicit because the SDK gives a test no per-invocation hook: a call nobody collected from adds nothing to the log, and a call that published nothing adds nothing either. That is what lets an assertion over a log mean "not anywhere in this test" rather than "not in the last call".

| Method | Gives |
|--------|-------|
| `EventLog::new(&env)` | An empty log |
| `log.collect()` | Appends the events of the invocation that just ran |
| `log.matcher()` | An `EventMatcher` over everything collected so far |
| `log.topics()` | `Vec<Vec<String>>` — one entry per event, its topic symbols |
| `log.events()` | The collected `ContractEvent` values |
| `log.len()` / `log.is_empty()` | How much has been gathered |

Events stay in call order — one contiguous run per collected invocation, in the order the contract published them — so `topics()` is what a sequence assertion reads:

```rust
let sequence = log
    .topics()
    .iter()
    .map(|topics| topics.join("."))
    .collect::<Vec<_>>()
    .join("|");
assert_eq!(sequence, "Transfer|CloseOffer");
```

A failure over a log reports the aggregate, which is the count of calls the assertion actually saw:

```text
assertion `left == right` failed: Expected 3 events, found 2
panicked at 'Expected an event whose data matches, checked 2 event(s) with data [1, 2]'
```

## Negative assertions

Proving something did *not* happen is the other half of event testing — a rejected transfer should emit no `Transfer` event, a closed offer should publish nothing at all:

```rust
use soroban_testkit_assert::events::EventMatcher;

// Nothing in scope was published
EventMatcher::new(&env).assert_not_emitted();

// A specific topic stayed silent
EventMatcher::new(&env)
    .from_contract(&contract_id)
    .with_topic("Transfer")
    .assert_not_emitted();

// Anything the topic filter cannot express
EventMatcher::new(&env).assert_none_match(|event| {
    matches!(event.type_, soroban_sdk::xdr::ContractEventType::Diagnostic)
});
```

`assert_none_match` hands you the raw `ContractEvent`, so a predicate can inspect the event type or the data payload rather than only the topics.

Both failures quote the event they were not supposed to find:

```text
panicked at 'Expected no events to be emitted, found 1 — unexpected event: topics [Transfer], type Contract'
```

<div class="tk-callout">
  <span class="tk-callout__title">A negative assertion reads its matcher's scope</span>
  <p><code>EventMatcher::new(&amp;env).assert_not_emitted()</code> proves the <strong>most recent invocation</strong> published nothing matching. Proving a topic never appeared across a test is the <a href="#asserting-across-several-invocations"><code>EventLog</code></a> case: collect each call, then <code>log.matcher().with_topic("Refund").assert_not_emitted()</code>.</p>
</div>

## Event count ranges and topic frequencies

An exact `assert_count(n)` breaks the day a batch call starts emitting one extra fee or audit event, even though the behaviour under test did not change. Two macros state what the test actually depends on instead: that the total falls in a range, and that a given topic appeared a given number of times.

```rust
use soroban_testkit_assert::{assert_event_count_range, assert_event_topic_count};

client.batch_transfer(&from, &recipients); // 2 transfers + auxiliary events

// Total events within a range: any standard range works.
assert_event_count_range!(env, 3..=5);
assert_event_count_range!(env, 2..);
assert_event_count_range!(env, ..=10);

// An exact topic frequency, or a range of them.
assert_event_topic_count!(env, "transfer", 2);
assert_event_topic_count!(env, "fee", 0..=1);
assert_event_topic_count!(env, "refund", 0);

// Restrict either one to the events of one contract.
assert_event_count_range!(env, from = token_id, 3..);
assert_event_topic_count!(env, from = token_id, "transfer", 2);
```

The bound is an exact count (`2`) or any range over integers: `a..b`, `a..=b`, `a..`, `..b`, `..=b`, or `..`. Integer literals need no suffix. A topic counts the same way `with_topic` matches: an event counts once when **any** of its topics is that symbol, and symbols are case sensitive.

The first argument is anything that implements `EventSource`: an `Env` (the most recent invocation) or an [`EventLog`](#asserting-across-several-invocations) (every call it collected), by value or by reference. That makes the macros the natural way to bound a whole multi-step test:

```rust
let mut log = EventLog::new(&env);
for batch in batches {
    client.batch_transfer(&from, &batch);
    log.collect();
}
assert_event_count_range!(log, 6..=12);
assert_event_topic_count!(log, "transfer", 6);
```

Both checks also exist as matcher methods, which compose with the existing filters:

```rust
EventMatcher::new(&env)
    .from_contract(&token_id)
    .with_topic("transfer")
    .assert_count_within(2..=3);

EventMatcher::new(&env).assert_topic_count("fee", 1);
```

### Failure diagnostics

A failure prints the bound, the observed count, the scope that was read, and a table of every topic signature in scope with how many events carried it, so you see what was emitted instead. Symbol topics are spelled out. Every other topic shows as its type (`<address>`, `<i128>`, …), so transfers to different recipients share one row:

```text
Event topic count mismatch: expected topic "Transfer" exactly 2 time(s), observed 0 (topic "Transfer" was not observed)
Scope: most recent invocation
| Observed topics     | Count |
|---------------------|-------|
| transfer, <address> |     2 |
| fee                 |     1 |
| audit               |     1 |
| total               |     4 |
```

The table lists signatures in the order they first appeared. It honours `from = …` / `from_contract` but deliberately ignores the topic being counted, because the useful question on failure is "what did come out of the call?".

## Asserting on payload data

Topics say which event fired; the data says what happened. `assert_data_matches` hands the predicate one `EventData` per event in scope, after the contract and topic filters:

```rust
use soroban_testkit_assert::events::EventMatcher;

// A single-value payload, read as the type it is
EventMatcher::new(&env)
    .with_topic("transfer")
    .assert_data_matches(|data| data.deserialize::<i128>() == Some(1_000));

// A struct published with `contractevent` arrives as a map keyed by field name
EventMatcher::new(&env)
    .from_contract(&contract_id)
    .with_topic("incremented")
    .assert_data_matches(|data| {
        data.field("new_count")
            .and_then(|value| value.deserialize::<u32>())
            == Some(12)
    });

// A `contracttype` struct deserializes whole, when a test wants all of it
EventMatcher::new(&env)
    .with_topic("moved")
    .assert_data_matches(|data| data.deserialize::<Moved>() == Some(expected));

// Anything the typed views cannot express: the payload as the ledger stored it
EventMatcher::new(&env)
    .with_topic("flag")
    .assert_data_matches(|data| matches!(data.raw(), soroban_sdk::xdr::ScVal::Bool(true)));
```

| View | Returns | Use it when |
|------|---------|-------------|
| `deserialize::<T>()` | `Option<T>` | the payload is one value of a known type — a number, `Address`, `Vec`/`Map`, or a `contracttype` struct |
| `field("name")` | `Option<EventData>` | the payload is a map, which is what a `contractevent` struct becomes, and one field is the point |
| `fields()` | `Vec<String>` | the payload is a map and you want to enumerate the field names in ledger order |
| `raw()` | `&ScVal` | neither fits, or the test is about the shape itself |

`deserialize` yields `None` rather than panicking, so one predicate can probe a payload without assuming it. `field` composes with it: `data.field("amount").and_then(|value| value.deserialize::<i128>())`.

`fields()` returns the field names actually present on the ledger in order:

```rust
EventMatcher::new(&env)
    .with_topic("moved")
    .assert_data_matches(|data| data.fields() == ["amount", "to"]);
```

<div class="tk-callout tk-callout--warn">
  <span class="tk-callout__title">A <code>contractevent</code> struct is not a <code>contracttype</code> struct</span>
  <p>The struct declared with <code>contractevent</code> gets an <code>Event</code> implementation, not ledger conversions, so <code>deserialize::&lt;Incremented&gt;()</code> will not compile against it. Read its fields with <code>field</code>. Note too that the SDK publishes event maps <strong>sparse</strong> by default: a field whose value is <code>None</code> is absent from the map rather than stored as void, so <code>field</code> returning <code>None</code> covers both "no such field" and "field left empty".</p>
</div>

A failed data assertion prints the payloads it was handed, which is the difference between a message you can read and a re-run:

```text
panicked at 'Expected an event whose data matches, checked 4 event(s) with data [7, 9000, true, "widget"]'
panicked at 'Expected an event whose data matches, checked 1 event(s) with data [{"amount": 500, "to": Contract(CAAAAAA...FCT4)}]'
```

Scalars print their values, maps and vectors print their entries, an address prints as its strkey (abbreviated above, printed whole in a real message), and a shape too rare to spell out falls back to its XDR debug form, so the message never hides the value it rejected. When no event is in scope the message says so instead of claiming the data was wrong.

### Checking an event payload schema

`assert_event_data_schema!` validates the complete map payload of every event
matching the topic. It reports missing or mistyped fields individually and
rejects extra keys, which helps catch accidental indexer schema changes:

```rust
use soroban_testkit_assert::assert_event_data_schema;
use soroban_sdk::Address;

client.transfer(&from, &to, &100);

assert_event_data_schema!(&env, "transfer", {
    amount: i128,
    from: Address,
    to: Address,
});
```

Types are checked by Soroban value deserialization, including `i128`, `u64`,
`Address`, `Symbol`, and `bool`. A failure names each offending field and shows
the expected type and observed type/value. The assertion requires at least one
event matching the topic.

## Successful contract calls

Use the generated client's `try_*` call with these macros instead of a bare
`unwrap()`. A host failure is decoded into its category, code, and known name
(for example `storage/3: MissingValue`); contract-specific errors retain their
code and are identified as unregistered unless decoded separately:

```rust
use soroban_testkit_assert::{
    assert_call_success_with_value, assert_no_contract_error,
};

// Assert success and keep the returned value for further checks.
let balance = assert_no_contract_error!(client.try_balance(&account));
assert!(balance > 0);

// Assert that a successful invocation returned exactly the expected value.
assert_call_success_with_value!(client.try_transfer(&from, &to, &100), ());
```

Both macros evaluate the call once and distinguish a returned contract/host
error from an invocation failure. `assert_no_contract_error!` returns the
successful value; `assert_call_success_with_value!` compares it with the
expected value.

## Asserting exact contract and host errors

`#[should_panic]` passes for *any* panic, so a test written to prove
"unauthorized callers are refused" keeps passing when the call starts failing
on a budget overflow instead. `assert_contract_error!` takes the result of a
`try_*` client call and fails unless it carries exactly the expected variant:

```rust
use soroban_testkit_assert::assert_contract_error;

assert_contract_error!(
    client.try_transfer(&sender, &receiver, &1000),
    TokenError::InsufficientBalance
);
```

It handles every arm of the SDK v28 `try_*` shape,
`Result<Result<T, ConversionError>, Result<E, InvokeError>>`:

| The call returned | The macro |
|-------------------|-----------|
| `Err(Ok(e))` with `e == expected` | passes |
| `Err(Ok(e))` with another variant | fails, naming both variants with code and category |
| `Err(Err(InvokeError::Contract(code)))` | fails: a code the client's error enum has no variant for |
| `Err(Err(InvokeError::Abort))` | fails: a panic or host failure |
| `Ok(..)` | fails: the call unexpectedly succeeded, printing the returned value |

```text
assert_contract_error! failed: the call failed with a different error
  expected: InsufficientBalance (code 1, category contract)
  received: Unauthorized (code 2, category contract)
  decoded:  Soroban Error [2]: unregistered contract error — add it to an ErrorRegistry to read it as text
```

The expected value can be the contract's error enum or a raw
`soroban_sdk::Error::from_contract_error(code)`; codes and categories come
from `DecodedError` in `soroban-testkit-core`.

### Host errors

`assert_host_error!` matches a host error at the precision you need:

```rust
use soroban_sdk::xdr::{ScErrorCode, ScErrorType};
use soroban_testkit_assert::assert_host_error;

assert_host_error!(outcome, ScErrorType::Storage);                             // any storage error
assert_host_error!(outcome, ScErrorType::Storage, ScErrorCode::MissingValue);  // exact category and code
assert_host_error!(outcome, Error::from_contract_error(101));                  // an exact Error value
```

The last form is how to assert an error code that propagated from a
*different* contract — a token's `InsufficientBalance` surfacing through a
pool whose client only knows `AmmError`.

Three things the host decides, not the macro:

- A `#[contracterror]` client can only carry contract codes; a host failure
  arrives as `InvokeError::Abort` with its category lost. The diagnostic says
  so — call `env.try_invoke_contract::<_, soroban_sdk::Error>(..)` to keep it.
- A host failure inside a native (non-Wasm) test contract surfaces as
  `context/InvalidAction`: the host catches it as a failed contract frame.
- Budget exhaustion is unrecoverable and escalates to a panic even through
  `try_*`; catch it with `#[should_panic(expected = "Error(Budget, ExceededLimit)")]`.

`check_contract_error` and `check_host_error` return the diagnostic as a
`Result<(), String>` instead of panicking.

## Authorization matching

Verify who had to sign for a call, and for what. A recorded authorization carries
the invoker, the contract function, and the arguments `require_auth` demanded, and
the macro names all three:

```rust
use soroban_testkit_assert::assert_auth_matches;

client.increment(&caller, &1);

// The invoker and the function are enough when the arguments are not the point.
assert_auth_matches!(env, {
    invoker: caller,
    function: "increment",
});

// Arguments are matched position by position; `_` accepts any value.
assert_auth_matches!(env, {
    invoker: caller,
    function: "increment",
    args: [_, 1u32],
});
```

<div class="tk-callout">
  <span class="tk-callout__title">Scope: the invocation that just ran</span>
  <p><code>assert_auth_matches!</code> reads <code>env.auths()</code>, which in SDK v28 reports the authorizations of the <strong>most recent contract invocation</strong> — the same scope as <code>EventMatcher::new</code>. Assert right after the call, or provide the expected auth with <code>env.mock_auths(..)</code> / <code>env.set_auths(..)</code> and let the call run with real requirements instead of <code>mock_all_auths()</code>.</p>
</div>

The matcher behind the macro is available directly when a test builds the filter
in steps:

```rust
use soroban_testkit_assert::auth::{AuthArgs, AuthMatcher};

AuthMatcher::new(&env)
    .invoker(&admin)
    .function("set_admin")
    .assert_matches();

AuthMatcher::new(&env)
    .args(AuthArgs::new().any().value(&env, 7u32))
    .assert_matches();
```

And the coarser questions still have their own assertions:

```rust
use soroban_testkit_assert::auth::AuthMatcher;

// Assert no auth was required
AuthMatcher::new(&env).assert_no_auth_required();

// Assert specific auth count
AuthMatcher::new(&env).assert_auth_count(2);
```

A failed match prints the expectation next to every authorization the invocation
recorded, so the wrong invoker, function, or argument is visible without a re-run:

```text
panicked at 'Expected an authorization matching { invoker Contract(C…FCT4), function "set_admin", args [_, 7] }, found 1 recorded authorization(s):
  [0] invoker Contract(C…FCT4) contract Contract(C…D2KM) function "increment" args [Contract(C…FCT4), 1]'
```

### Sub-tree authorization matching

Multi-hop routing protocols (such as `aggregator -> pool -> vault`) record nested
`AuthorizedInvocation` trees in `env.auths()`. `assert_auth_sub_tree_matches!`
locates and validates a sub-tree starting at any depth, so a modular test can
verify an intermediate contract hop without repeating the entire outer call stack:

```rust
use soroban_testkit_assert::assert_auth_sub_tree_matches;

aggregator.swap(&user, &pool_id, &vault_id, &500i128);

// Matches Pool -> Vault inside Aggregator -> Pool -> Vault without naming Aggregator.
assert_auth_sub_tree_matches!(env, {
    invoker: user,
    contract: pool_id,
    function: "route",
    args: [_, vault_id.clone(), 500i128],
    sub_invocations: [
        {
            contract: vault_id,
            function: "deposit",
            args: [user.clone(), 500i128],
            sub_invocations: [],
        },
    ],
});
```

Every field (`invoker`, `contract`, `function`, `args`, `sub_invocations`) is
optional and accepts `_` as a wildcard. Inside `sub_invocations: [..]`:

| Pattern | Matches |
|---------|---------|
| Omitted or `sub_invocations: _` | Any child invocations (zero or more) |
| `sub_invocations: []` | Exactly zero child invocations (leaf node) |
| `_` | Any single child invocation at that position |
| `..` | Zero or more child invocations at that position |
| `{ ... }` | A nested sub-tree node with its own filters and `sub_invocations` |

The same matching is available through the chainable builder types
`AuthSubTree`, `SubInvocations`, and `AuthSubTreeMatcher`:

```rust
use soroban_testkit_assert::auth::{AuthArgs, AuthSubTree, AuthSubTreeMatcher, SubInvocations};

let expected = AuthSubTree::new()
    .contract(&pool_id)
    .function("route_with_fee")
    .sub_invocations(
        SubInvocations::new()
            .node(
                AuthSubTree::new()
                    .contract(&vault_id)
                    .function("deposit")
                    .args(AuthArgs::new().any().value(&env, 1_000i128)),
            )
            .any_many(),
    );

AuthSubTreeMatcher::new(&env)
    .invoker(&user)
    .sub_tree(expected)
    .assert_matches();
```

When a sub-tree fails to match, the assertion renders a side-by-side tree
comparison highlighting missing branches with `[MISSING]`:

```text
Expected authorization sub-tree did not match any recorded authorization (1 recorded):

Expected sub-tree                                            | Recorded authorization tree(s)
-------------------------------------------------------------+-------------------------------------------------------------
invoker Contract(C…1234)                                     | [0] invoker Contract(C…1234)
contract Contract(C…BBBB) function "route"                   | contract Contract(C…AAAA) function "swap" args [...]
├── contract Contract(C…CCCC) function "deposit"             | └── contract Contract(C…BBBB) function "route" args [...]
└── contract Contract(C…DDDD) function "collect" [MISSING]   |     └── contract Contract(C…CCCC) function "deposit" args [...]
```

### Isolating intermediate sub-invocations

When inspecting authorization trees from complex multi-contract calls, developers often only care about asserting that a specific intermediate contract properly authorized a sub-call, regardless of top-level invoker details.

`assert_auth_sub_invocation!` searches recursively across all recorded authorization trees and matches any sub-invocation matching `{ contract, function, args }` at any depth:

```rust
use soroban_testkit_assert::assert_auth_sub_invocation;

aggregator.swap(&user, &pool_id, &vault_id, &500i128);

// Matches Vault deposit at depth 2 without naming Aggregator or Pool:
assert_auth_sub_invocation!(env, {
    contract: vault_id,
    function: "deposit",
    args: [user.clone(), 500i128],
});

// Matches intermediate Pool route without top-level wrapper details:
assert_auth_sub_invocation!(env, {
    contract: pool_id,
    function: "route",
    args: [_, vault_id, 500i128],
});
```

All fields are optional and accept `_` as a wildcard:

- `contract: addr` (or `_` / omitted): filters calls targeting `addr`.
- `function: "name"` (or `_` / omitted): filters calls named `"name"`.
- `args: [_, val]` (or `_` / omitted): matches arguments positionally with `_` wildcards.
- `invoker: addr` (or `_` / omitted): restricts matches to authorization trees originating from `addr`.

The same matching is available via the chainable builder `AuthSubInvocationMatcher`:

```rust
use soroban_testkit_assert::auth::{AuthArgs, AuthSubInvocationMatcher};

AuthSubInvocationMatcher::new(&env)
    .contract(&vault_id)
    .function("deposit")
    .args(AuthArgs::new().any().value(&env, 500i128))
    .assert_matches();
```

When no matching sub-invocation is found, the assertion formats the full recorded authorization tree displaying all sub-invocations:

```text
panicked at 'Expected authorization sub-invocation matching { contract Contract(C…), function "deposit", args [_, 999] }, found no match in 1 recorded authorization tree(s):
[0] invoker Contract(C…1234)
contract Contract(C…AAAA) function "swap" args [...]
└── contract Contract(C…BBBB) function "route" args [...]
    └── contract Contract(C…CCCC) function "deposit" args [...]'
```

## Failure messages

Assertions fail with the counts they observed, which is what makes them worth using over hand-rolled loops:

```text
assertion `left == right` failed: Expected 3 events, found 1
```

## Storage tier quota assertions

Protocols must keep storage tier entry counts within bounded budgets: instance storage is reserved for small singleton configuration, persistent storage holds durable state, and temporary storage manages short-lived queues. `assert_storage_tier_count!` validates entry count quotas per tier:

```rust
use soroban_testkit_assert::assert_storage_tier_count;

// Enforce quotas using block syntax:
assert_storage_tier_count!(env, contract_id, {
    instance: 1,           // Exactly 1 instance entry
    max_persistent: 10,    // At most 10 persistent entries
    temporary: 0,          // No temporary storage entries
});

// Or concise assignment syntax:
assert_storage_tier_count!(env, contract_id, max_instance = 3, max_persistent = 10);
```

When a quota is breached, the assertion prints an informative breakdown table listing all stored keys per tier:

```text
Storage allocation quota exceeded for contract:
Breached tiers: persistent (found 12, expected max 10)

| Tier        | Count | Quota          | Status | Keys                                       |
|-------------|-------|----------------|--------|--------------------------------------------|
| instance    | 1     | max 5          | PASS   | "cfg"                                      |
| persistent  | 12    | max 10         | BREACH | "u_1", "u_2", "u_3", ... (total 12)        |
| temporary   | 0     | exact 0        | PASS   | (none)                                     |
```

## Multi-party balance conservation

Asserting balance conservation across multi-user operations (e.g., swaps, multi-hop routes, fee splits, or batch transfers) commonly leads to repetitive boilerplate when querying balances before and after calls. The `assert_balance_conserved!` macro captures pre-invocation balances, runs the operation, verifies that total tokens across all tracked participants plus reserves are conserved, and accounts for explicit external mints or burns:

```rust
use soroban_testkit_assert::assert_balance_conserved;

// Standard 2-party transfer:
assert_balance_conserved!(
    accounts: [&alice, &bob],
    balance: |addr| token_client.balance(addr),
    call: {
        token_client.transfer(&alice, &bob, &100);
    }
);

// Multi-party swap with fees, liquidity pool reserve, and custom labels:
assert_balance_conserved!(
    accounts: [
        (&alice, "Alice (Trader)"),
        (&bob, "Bob (Recipient)"),
        (&pool, "AMM Liquidity Pool"),
        (&treasury, "Protocol Fee Vault")
    ],
    balance: |addr| token_client.balance(addr),
    call: {
        router.swap(&alice, &bob, &1000);
    }
);

// Invocations involving external mints or burns:
assert_balance_conserved!(
    accounts: [&alice, &vault],
    balance: |addr| token_client.balance(addr),
    mint: 500,
    burn: 100,
    call: {
        token_client.mint(&alice, &500);
        token_client.burn(&vault, &100);
    }
);
```

When an invariant is violated (for example, if a fee calculation suffers from fractional token loss or round-off leakage), `assert_balance_conserved!` panics with a formatted debit/credit reconciliation table:

```text
Balance Conservation Breach: Discrepancy = -1 (LEAK DETECTED)
Expected net delta: 0 (mints: +0, burns: -0) | Actual net delta: -1

------------------------------------------------------------------------------------------------------
| Account / Reserve            |    Pre-Balance |   Post-Balance |     Delta (Debit/Credit) |
------------------------------------------------------------------------------------------------------
| Alice                        |           1000 |            900 |                     -100 |
| Bob                          |              0 |             97 |                      +97 |
| Fee Collector                |              0 |              2 |                       +2 |
------------------------------------------------------------------------------------------------------
| Total Tracked                |           1000 |            999 |                       -1 |
| External Mints (+)           |                |                |                       +0 |
| External Burns (-)           |                |                |                       -0 |
------------------------------------------------------------------------------------------------------
| Discrepancy                  |                |                |       -1 (LEAK DETECTED) |
------------------------------------------------------------------------------------------------------
```

## Comparative budget differential and relative gas savings

When refactoring code paths or evaluating alternative algorithm implementations (such as linear scan vs binary search, or unindexed vs indexed storage queries), smart contract engineers need to assert that an optimized branch achieves relative gas savings without regressing on CPU instruction or memory consumption.

The `assert_budget_diff_within!` macro executes baseline and optimized closures, measures the differential CPU instructions and memory bytes consumed, and asserts that relative savings targets are met.

### Basic comparison

In default mode, the optimized branch must consume strictly fewer instructions or memory bytes than baseline and cannot regress in either dimension:

```rust
use soroban_testkit_assert::assert_budget_diff_within;

assert_budget_diff_within!(
    &env,
    baseline: {
        client.linear_search(&items, &target);
    },
    optimized: {
        client.binary_search(&items, &target);
    },
);
```

### Enforcing relative and absolute savings

Specify target percentage reductions (`min_cpu_savings_percent`, `min_mem_savings_percent`, `min_savings_percent`) or absolute thresholds (`min_cpu_savings`, `min_mem_savings`):

```rust
assert_budget_diff_within!(
    env: &env,
    baseline: || client.unindexed_scan(&key),
    optimized: || client.indexed_lookup(&key),
    min_cpu_savings_percent: 25.0, // At least 25% CPU instruction reduction
    min_mem_savings_percent: 10.0, // At least 10% memory byte reduction
);
```

### Space-time tradeoffs and dual environments

For operations that modify contract state or trade slight memory increases for massive CPU reductions:

```rust
// Separate environments ensure identical initial state:
assert_budget_diff_within!(
    baseline: (&env1, { client1.bubble_sort(&data) }),
    optimized: (&env2, { client2.radix_sort(&data) }),
    min_cpu_savings_percent: 50.0,
    allow_mem_regression: true, // Permit memory increase for large CPU reduction
);
```

### Side-by-side consumption table on failure

When an optimized branch regresses or fails to reach the required savings target, `assert_budget_diff_within!` panics with a formatted side-by-side consumption table:

```text
Budget Differential Assertion Failed!
Optimized branch failed to achieve required gas savings or exceeded baseline limits.

======================================================================================================================================
| Metric             |         Baseline |        Optimized |   Delta (Opt-Base) |          Savings (%) | Target           |  Status  |
======================================================================================================================================
| CPU Instructions   |          100,000 |           85,000 |            -15,000 |      +15.00% savings | >= 25.00%        |   FAIL   |
--------------------------------------------------------------------------------------------------------------------------------------
| Memory (Bytes)     |            4,096 |            3,840 |               -256 |       +6.25% savings | <= baseline      |   PASS   |
======================================================================================================================================

Failure Summary:
  • Failed to achieve target CPU savings: achieved 15.00%, expected at least 25.00%
```

<div class="tk-grid tk-grid--2">
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Next step</span>
    <h3 class="tk-card__title">Filtering by contract</h3>
    <p class="tk-card__body"><code>from_contract</code> narrows a matcher to one contract id — essential once a test wires several contracts together.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">Guide</span>
    <h3 class="tk-card__title">Testing patterns</h3>
    <p class="tk-card__body">See <a href="../guides/testing-patterns.html">Testing Patterns</a> for event and auth assertions in multi-contract suites.</p>
  </div>
</div>

## Budget ranges and ceilings (`assert_budget_consumed!`)

`assert_budget_consumed!` bounds what the most recent top-level invocation cost (`BudgetSnapshot::last_invocation`), without manual snapshot arithmetic. Each metric takes either a range or an inclusive ceiling:

```rust
use soroban_testkit_assert::assert_budget_consumed;

client.swap(&user, &100);

assert_budget_consumed!(env, {
    max_cpu: 150_000,
    max_mem: 50_000,
});

assert_budget_consumed!(env, {
    cpu: 10_000..=15_000, // any range: a..b, a..=b, a.., ..b, ..=b
    mem: 1_000..,
});
```

| Key | Value | Meaning |
| --- | --- | --- |
| `cpu` | `u64` range | CPU instructions must fall inside the range |
| `max_cpu` | `u64` | CPU instructions must not exceed it (inclusive) |
| `mem` | `u64` range | memory bytes must fall inside the range |
| `max_mem` | `u64` | memory bytes must not exceed it (inclusive) |

An unknown key is a compile error. On failure every breached metric is reported at once, with the observed value, the expected bound, the miss, and the miss as a percentage of the crossed bound:

```text
assert_budget_consumed! failed:
  cpu_insns: observed 1_650, expected ..=1_500 — over the upper bound by 150 (+10.00%)
  observed totals: cpu_insns=1_650 mem_bytes=812
```

The same check is available without the macro: build a `BudgetBounds` (`.cpu(..)`, `.max_cpu(..)`, `.mem(..)`, `.max_mem(..)`) and call `check(&snapshot)` or `assert_within(&snapshot)` on any `BudgetSnapshot`.
