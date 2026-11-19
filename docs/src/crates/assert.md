# soroban-testkit-assert

Fluent matchers for contract events and authorizations, so assertions read like the behaviour you expect.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-assert</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">events · auth</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Style</span><span class="tk-spec__value">chainable builder</span></div>
</div>

## API at a glance

| Method | Asserts |
|--------|---------|
| `EventMatcher::assert_emitted()` | At least one matching event exists |
| `EventMatcher::assert_not_emitted()` | No matching event exists |
| `EventMatcher::assert_none_match(pred)` | No matching event satisfies a predicate |
| `EventMatcher::assert_count(n)` | Exactly `n` events were emitted |
| `EventMatcher::assert_data_matches(pred)` | One event's payload satisfies a predicate |
| `EventMatcher::from_contract(addr)` | Restricts matches to one contract address |
| `EventMatcher::with_topic(t)` | Restricts matches to events carrying that topic symbol |
| `EventLog::collect()` | Gathers one invocation's events into a set spanning calls |
| `EventLog::matcher()` | A matcher over everything that log collected |
| `AuthMatcher::assert_no_auth_required()` | The call needed no authorizations |
| `AuthMatcher::assert_auth_count(n)` | Exactly `n` authorizations were recorded |
| `AuthMatcher::assert_matches()` | One recorded authorization fits the configured invoker, function, and arguments |
| `assert_auth_matches!(env, { .. })` | The same, with the invoker, function, and argument pattern inline |
| `AuthSubTreeMatcher::assert_matches()` | A matching authorization sub-tree appears at any depth in recorded auths |
| `assert_auth_sub_tree_matches!(env, { .. })` | Inline sub-tree matcher with argument and child wildcards and side-by-side failure trees |
| `StorageQuotaMatcher::assert_matches()` | Asserts storage tier counts satisfy exact quotas or upper bounds |
| `assert_storage_tier_count!(env, contract, { .. })` | Macro enforcing allocation quotas across instance, persistent, and temporary tiers |

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
