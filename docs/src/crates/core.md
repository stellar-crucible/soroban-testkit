# soroban-testkit-core

Budget tracking, error decoding, storage inspection and contract dependency graphs — the primitives the rest of Testkit is built on.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-core</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">budget · error · storage · dependency · footprint · reentrancy</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Dependencies</span><span class="tk-spec__value">soroban-sdk · soroban-env-host · serde · serde_json</span></div>
</div>

## API at a glance

| Type | Purpose |
|------|---------|
| `BudgetSnapshot` | CPU instructions and memory bytes — `last_invocation()` for the call that just ran |
| `BudgetRead` | Trait implemented by any source `capture()` can read: invocation resources, the SDK budget, the host `Budget` |
| `BudgetGuard` | Ceilings and a baseline tolerance for one named operation |
| `BudgetViolation`, `ViolationKind` | The limit a cost broke, rendered as one parseable line |
| `BudgetMetric` | Which resource a violation was measured against — `Cpu` or `Memory`, and the `cpu_insns` / `mem_bytes` word `as_str()` prints for it |
| `BudgetBaseline` | Recorded costs per case, loaded from and saved to a JSON file |
| `budget_guard!` | Macro form of the guard around a single invocation |
| `DecodedError` | A Soroban error code split into category, meaning and context |
| `ErrorRegistry` | Your `#[contracterror]` codes mapped to the words they mean |
| `ClientOutcome`, `unwrap_decoded` | Unwrap a v28 `try_*` client call, panicking with the decoded error |
| `StorageEntry`, `StorageTier` | A live storage key, its rendered value, tier and expiry ledger |
| `StorageSnapshot` | Every live entry of one contract, captured at a point in time |
| `StorageDiff`, `StorageChange` | Keys added, removed and rewritten between two snapshots |
| `FootprintSnapshot` | XDR byte size of every live storage key of one contract, by tier |
| `RentParameters`, `RentEstimate` | Protocol fee parameters and the Stroop cost of keeping a footprint alive for N ledgers |
| `StorageFootprint` | Footprints for several contracts, loaded from and saved to a JSON artifact |
| `StorageFootprintDiff` | Base vs head footprints: growing keys, a Markdown report, a growth threshold |
| `DependencyGraph` | Directed graph of inter-contract calls, built from an invocation's `fn_call` diagnostics |
| `DependencyEdge` | One caller → callee edge with its call count and functions |
| `DependencyCycle` | A closed call chain such as `A -> B -> C -> A`, with self-recursion as `A -> A` |
| `storage-footprint-diff` | Binary wrapping `StorageFootprintDiff` for CI |
| `StorageFragmentationAnalyzer` | Analyzes key-to-value serialization overhead, detects fragmentation, and estimates Stroop savings |
| `FragmentationReport` | Comprehensive metrics across tiers, high-overhead alerts, and Markdown recommendations |
| `StackDepthAuditor` | Reconstructs a contract call chain from host diagnostic events and checks it against a depth cap |
| `CallFrame` | One invocation in that chain: depth, contract id, function |
| `StackDepthBreach` | A trace deeper than its cap, rendered with every frame |
| `CompatibilityAuditor` | Evaluates contracts against upcoming SDK v29 / Protocol 22+ changes and flags deprecated host calls |
| `CompatibilityScorecard` | Actionable readiness scorecard with grades, remediation advice, and Markdown output |
| `ReentrancyGuardAuditor` | Statically and dynamically analyzes lock flag state across multi-frame calls and external callbacks |
| `LockState` | `Uninitialized`, `Locked`, or `Unlocked` reentrancy guard states |
| `ReentrancyReport` | Comprehensive violation diagnostics and visual ASCII state access ordering timelines |
| `ReentrancyViolation` | Flagged uninitialized, unlocked, premature release, dangling, or indirect reentrant calls |

## Budget snapshots

The SDK meters one top-level invocation at a time, and the reading a test wants
is usually the call that just finished:

```rust
use soroban_testkit_core::budget::BudgetSnapshot;

client.increment(&caller, &1);
let cost = BudgetSnapshot::last_invocation(&env);

println!("CPU instructions: {}", cost.cpu_insns);
println!("Memory bytes: {}", cost.mem_bytes);
```

`capture()` takes anything implementing `BudgetRead` — those invocation
resources, the cumulative `env.cost_estimate().budget()`, and a
`soroban_env_host::budget::Budget` you charge by hand. `diff()` saturates at
zero, so a later snapshot that reads smaller never underflows an assertion.

<div class="tk-callout">
  <span class="tk-callout__title">SDK v28 note</span>
  <p><code>env.budget()</code> is deprecated; go through <code>env.cost_estimate()</code>. And read the numbers as a comparison between builds rather than as a fee quote: a contract registered as a native test contract is never metered through the VM, so wasm instantiation, execution and rent reads are absent from the reading.</p>
</div>

## Guards: turning a measurement into a limit

A number on its own passes whatever it is. `BudgetGuard` states the two things a
test can actually act on — an absolute ceiling, and how far the cost may have
grown relative to the last time it was recorded.

```rust
use soroban_testkit_core::budget::{BudgetGuard, BudgetSnapshot};

let cost = BudgetSnapshot { cpu_insns: 1_500_000, mem_bytes: 200_000 };

BudgetGuard::new("transfer")          // the name every failure line carries
    .cpu_ceiling(2_000_000)
    .mem_ceiling(500_000)
    .baseline(Some(previous_cost))    // `None` for a case not recorded yet
    .tolerance_percent(10)            // 0 means "not one instruction more"
    .assert_within(&cost);
```

`run()` makes the call, reads the metering that call left behind, and returns
whatever the invocation returned:

```rust
let total = BudgetGuard::new("increment")
    .cpu_ceiling(20_000_000)
    .run(&env, || client.increment(&caller, &1));
```

The `budget_guard!` macro is the same call written around the invocation, with
the limits in a block:

```rust
use soroban_testkit_core::budget_guard;

budget_guard!(&env, "increment", { cpu_max: 20_000_000, mem_max: 20_000_000 }, || {
    client.increment(&caller, &1)
});
```

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Every breach, not just the first</span>
  <p><code>violations()</code> returns all limits one cost breaks — ceilings first, then growth — and <code>assert_within()</code> panics with one line per breach. A change that costs both CPU and memory is found in one run, and the lines are <code>key=value</code> pairs, so CI can grep them.</p>
</div>

```text
BUDGET kind=growth case=transfer metric=cpu_insns actual=1500000 limit=1100000 baseline=1000000 tolerance_percent=10
```

## Baselines: recording cost so a regression is visible

`BudgetBaseline` is a map of case name to recorded cost that reads and writes a
JSON file, so the numbers a suite enforces live in the repository rather than in
a test's memory.

```rust
use soroban_testkit_core::budget::BudgetBaseline;

let baseline = BudgetBaseline::load(std::path::Path::new("tests/budget.json"))?;

for (case, cost) in baseline.cases() {
    baseline.guard(case).tolerance_percent(5).run(&env, || invoke(case));
}
```

`load` and `save` are the file-shaped ends; `parse` and `to_json_string` do the same work on a `&str` and a `String`, which is what a CI job that reads the baseline from an artifact needs. `get(case)` reads one recorded cost, `record(case, cost)` adds or replaces one and returns `&mut Self` so calls chain, and `len` / `is_empty` describe the map.

The file format is one version field and one map of costs:

```json
{
  "version": 1,
  "cases": {
    "increment": { "cpu_insns": 32669, "mem_bytes": 5252 }
  }
}
```

Cases are keyed by name and iterated in name order, so a rewritten file stays a
readable diff. A file whose `version` is newer than the loader is refused with
`BaselineError::UnsupportedVersion` rather than silently read with missing cases.

<div class="tk-callout">
  <span class="tk-callout__title">Recording a baseline</span>
  <p><code>record()</code> and <code>save()</code> write the file; what refreshes it is a policy choice. Committing the file and failing on drift keeps the numbers honest, so most projects regenerate it in one deliberate commit rather than on every test run — <a href="../guides/budget-testing.html">Budget-Aware Testing</a> walks through both. The pattern is applied in this repository: <code>examples/counter/budget.json</code> holds the counter's two hot paths, two <code>#[ignore]</code>d tests record and check it, and the <code>Budget baseline</code> workflow runs the check on one pinned runner and reports the numbers on the pull request.</p>
</div>

## Storage leaks

A contract that opens an order, escrow or position allocates keys for it and is
supposed to remove them when the entity closes. A forgotten `remove` does not
fail the call that forgot it — it quietly keeps paying rent. `StorageLeakDetector`
captures the contract's storage before a lifecycle and reports the persistent and
temporary keys that are still live at teardown.

```rust
use soroban_testkit_core::leak_detector::StorageLeakDetector;

let mut detector = StorageLeakDetector::start(&ctx.env, &contract_id);
// ... open escrow, fund, cancel ...
detector.checkpoint("cancel");

let leaks = detector.detect_orphaned_keys();
assert!(leaks.is_empty(), "Storage leak detected: {leaks:?}");
```

`detect_orphaned_keys()` returns a `Vec<StorageLeak>`, each carrying the key,
the storage tier, an estimated byte size (the rendered key and value) and the
label of the first checkpoint that saw the key. A leak renders as one readable
diagnostic:

```text
storage leak: temporary:quote (12 bytes, allocated at open escrow)
```

`assert_no_orphaned_keys()` fails with every leak on its own line, and
`report(&leaks)` returns the same text for a custom assertion message.

<div class="tk-callout">
  <span class="tk-callout__title">Only new, non-instance keys are leaks</span>
  <p>Keys present in the baseline snapshot are never reported: the detector judges what the lifecycle allocated, not what the contract already owned. Instance storage is the contract's singleton configuration and is always exempt. A key whose value was rewritten is not a leak either — the comparison is key identity.</p>
</div>

Permanent records a contract is supposed to keep — a config record, a factory
registry, a fee cache — are exempted by name or wildcard:

```rust
use soroban_testkit_core::StorageLeakDetector;

let detector = StorageLeakDetector::start(&env, &contract)
    .ignoring_key("config")          // exact key, any tier
    .ignoring_key("persistent:fees") // tier-qualified exact key
    .ignoring_pattern("order:*")     // a family of keys
    .ignoring_pattern("*:cache");    // a suffix on the qualified name

assert!(detector.detect_orphaned_keys().is_empty());
```

`checkpoint(label)` records the current storage as a phase of the lifecycle. The
first checkpoint containing a leaked key becomes its allocation site, so a
failure points at the step that should have released it instead of only naming
the key. Without a checkpoint a leak is still detected; its site reads as
unobserved. `detect_orphaned_keys_in(&snapshot)` runs the same analysis against
a snapshot the test captured itself.

## Error decoding

Soroban reports failures as a packed integer. `DecodedError::from_error` splits it into the category the host raised it in and the code's known meaning:

```rust
use soroban_testkit_core::error::DecodedError;

// A v28 client's `try_*` method reports the contract's own error as `Err(Ok(error))`.
let error = client.try_withdraw(&amount).unwrap_err().unwrap();

println!("{}", DecodedError::from_error(&error));
// Soroban Error [storage/3]: MissingValue — a required value was not provided (errors accessing host storage)
```

The ten categories the protocol defines (`contract`, `wasm_vm`, `context`, `storage`, `object`, `crypto`, `events`, `budget`, `value`, `auth`) and the ten standard codes (`ArithDomain`, `IndexBounds`, `InvalidInput`, `MissingValue`, `ExistingValue`, `ExceededLimit`, `InvalidAction`, `InternalError`, `UnexpectedType`, `UnexpectedSize`) are all described. An unrecognised number falls back to a sentence naming it rather than a panic.

### Your own error codes

A `#[contracterror]` enum hands the host an integer with no words attached, so the vocabulary is yours to supply. `ErrorRegistry` is that mapping:

```rust
use soroban_testkit_core::error::{ErrorRegistry, DecodedError};

let registry = ErrorRegistry::new()
    .register(101, "Insufficient balance")
    .register(102, "Contract is paused");

let decoded = DecodedError::from_error_with(&error, &registry);
assert_eq!(decoded.to_string(), "Soroban Error [101]: Insufficient balance");
```

Without a registry a contract code still decodes — it just says the code is unregistered, which beats reading `Error(3)` and going hunting.

### In test output

`unwrap_decoded` unwraps a v28 `try_*` client call and panics with the decoded sentence, so a failing call reads as a diagnosis in the test log rather than as nested `Result` debug output:

```rust
use soroban_testkit_core::error::unwrap_decoded;

let total: u32 = unwrap_decoded(client.try_withdraw(&amount));
// panicked at 'Soroban Error [budget/5]: ExceededLimit — a gas or size limit was hit (errors relating to budget limits)'
```

The nesting you see at that call site — `Err(Ok(error))` — is the SDK's own shape for a client method whose contract function returns `Result`; `ClientOutcome<T>` is the alias Testkit names it with. `unwrap_decoded_with` is the same call against an `ErrorRegistry`, so your own codes come out as words.

`DecodedError` implements `Display`, so it also reads well inside `assert!` messages:

```rust
use soroban_testkit_core::error::DecodedError;

let err = DecodedError {
    code: 12,
    category: "contract",
    message: "Insufficient balance".to_string(),
    context: Some("transfer".to_string()),
};
assert!(format!("{}", err).contains("Insufficient balance"));
```

The rest of the module is small. `DecodedError::from_contract_code(code)` decodes a bare `#[contracterror]` integer as read from `Error::get_code()`, with no error value to unpack, and `label()` renders a code the way the host does — `category/code` for a host error, the bare number for a contract one, which is why the output above reads `[12]` rather than `[contract/12]`. On the registry side, `lookup(code)` reads back a name you registered, `decoded_contract(code)` builds a `DecodedError` from it and says so plainly when the code is not registered, and `len` / `is_empty` describe the map. `CATEGORY_CONTRACT` is the exported constant for the `"contract"` category those compare against.

## Storage snapshots and diffs

Capture everything a contract holds, then compare two captures around an
operation. Keys and values are rendered to stable strings (`u32` becomes `"42"`,
a map becomes `"{a: 1, b: 2}"`), so a diff reports what changed rather than two
opaque blobs. Instance storage is unfolded entry by entry, and each entry
carries the ledger sequence it expires at.

```rust
use soroban_testkit_core::storage::{inspect_storage, StorageSnapshot, StorageTier};

let entries = inspect_storage(&env, &contract);
for entry in &entries {
    println!("{} = {} ({})", entry.qualified(), entry.value, entry.key);
}

let before = StorageSnapshot::capture(&env, &contract);
client.bump();
let after = StorageSnapshot::capture(&env, &contract);

let diff = before.diff(&after);
assert!(diff.added().is_empty() && diff.removed().is_empty());
assert_eq!(diff.modified()[0].after, "43");

// Or assert directly; failures list what actually happened.
before.assert_entry_removed(&after, "pending");
after.assert_unchanged(&StorageSnapshot::capture(&env, &contract));
assert_eq!(after.in_tier(StorageTier::Temporary).len(), 1);
```

### Storage tier assertions

Use `assert_storage_tier!` to assert that a key or pattern belongs to the expected [`StorageTier`]:

```rust
use soroban_testkit_core::assert_storage_tier;
use soroban_testkit_core::storage::StorageTier;

// Assert an exact key
assert_storage_tier!(&env, &contract, "admin", StorageTier::Instance);

// Assert all keys matching a wildcard pattern
assert_storage_tier!(&env, &contract, "balances:*", StorageTier::Persistent);
```

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Scope</span>
  <p>A snapshot covers one contract only: other contracts' data, the WASM <code>ContractCode</code> entry and ledger entries the contract does not own are never included. Account addresses own no contract data, so they capture as empty instead of failing.</p>
</div>

## Storage Namespace Auditing

When contracts compose multiple modules or traits, bare symbol keys such as
`"admin"` can collide and silently overwrite another module's state.
`NamespaceAuditor` records keys per module and reports collisions and
un-namespaced keys.

```rust
use soroban_testkit_core::namespace_auditor::NamespaceAuditor;

let mut auditor = NamespaceAuditor::new(&env, &contract);
auditor
    .record("ownable", "ownable::admin")
    .record("pausable", "pausable::paused");

let report = auditor.audit();
assert!(!report.has_collisions(), "Storage collision: {:?}", report.collisions());
println!("{report}");
```

Keys are considered namespaced when they contain a namespace separator
(`::`, `:`, `.`, `/`) such as `DataKey::Admin` or `ownable:admin`. Bare keys
like `admin` are flagged as collision risks. A collision is reported when the
same key is written by two independent modules. `report.modules()` returns the
full key map, while `NamespaceAuditor::from_entries` infers module ownership
from a `StorageEntry` snapshot.

## Storage footprint diffs in CI

A refactor that widens a struct or adds a key silently grows what a contract
keeps on the ledger, and with it state rent and every read/write fee. The
`footprint` module measures that growth in the unit those fees are charged in —
XDR-encoded bytes — and diffs it between two Git revisions.

A test records the footprint of each contract into a JSON artifact that the
repository commits:

```rust
use soroban_testkit_core::footprint::StorageFootprint;

#[test]
#[ignore = "run with --ignored to refresh storage-footprint.json"]
fn record_storage_footprint() {
    let (env, token) = deploy_and_exercise_token();
    let (_, vault) = deploy_and_exercise_vault(&env);

    let mut footprint = StorageFootprint::new();
    footprint.capture("token", &env, &token);
    footprint.capture("vault", &env, &vault);
    footprint.save(std::path::Path::new("storage-footprint.json")).unwrap();
}
```

```json
{
  "version": 1,
  "contracts": {
    "token": {
      "entries": { "instance:admin": 84, "persistent:balances": 200 }
    }
  }
}
```

Persistent and temporary keys are measured as their whole ledger entry; each
key inside instance storage is measured as its key-plus-value map entry. Keys
are `tier:key`, iterated in order, so the committed file diffs cleanly.

`StorageFootprintDiff::compare(&base, &head)` lines two artifacts up key by
key. `changed_keys()` lists every key whose size moved, largest growth first,
`growing_keys()` only the new or larger ones, and `exceeding(pct)` the
contracts whose *total* grew by more than `pct` percent. A contract that is new
in the head artifact is reported but not counted against the threshold, since
there is no base to grow from. `to_markdown(Some(pct))` renders the report:

```markdown
## Storage footprint

**Fail** — storage grew by more than 10% in `token` (+35.21%).

| Contract | Instance | Persistent | Temporary | Total | Change |
|----------|---------:|-----------:|----------:|------:|-------:|
| `token` | 84 | 200 → 260 | 0 → 40 | 284 → 384 | +35.21% |
| `vault` | 0 | 100 | 0 | 100 | 0% |

### Changed keys

| Contract | Key | Base | Head | Change |
|----------|-----|-----:|-----:|-------:|
| `token` | `persistent:balances` | 200 | 260 | **+60 B** |
| `token` | `temporary:nonce` | – | 40 | **+40 B** (new) |
```

### The `storage-footprint-diff` binary

```sh
storage-footprint-diff BASE.json HEAD.json [--threshold PCT] [--output FILE] [--allow-missing-base]
```

It prints the Markdown report, also writes it to `--output` for a comment
step, and exits `0` within the threshold, `1` when a contract grew past it, and
`2` on a usage or file error. `--allow-missing-base` treats an absent base file
as empty, for the pull request that introduces the artifact. The binary reads
files and never runs Git itself, so the base revision is whatever the job
extracts:

```yaml
- run: cargo test --workspace -- --ignored record_storage_footprint
- run: git show "origin/${BASE_REF}:storage-footprint.json" > base-footprint.json || true
  env:
    BASE_REF: ${{ github.base_ref }}
- run: >
    cargo run -p soroban-testkit-core --bin storage-footprint-diff --
    base-footprint.json storage-footprint.json
    --threshold 10 --output footprint.md --allow-missing-base
```

Post `footprint.md` as the pull-request comment the same way the `Budget
baseline` workflow posts its numbers. `run_diff_cli(args)` is the same binary
as a function, for embedding it in an `xtask` without spawning a process.

## Rent projection and the summary table

Byte counts answer "how much state is there". The question a deployer actually
asks is "what does keeping it cost", and the two differ by the entry writes and
the rent term the network charges per TTL bump. `FootprintSnapshot` projects
forward, and renders itself as a table.

`StorageFootprint::measure` captures one contract without going through an
artifact, which is what you want inside a test that just deployed something:

```rust
use soroban_testkit_core::{DEFAULT_PROJECTION_LEDGERS, StorageFootprint};

let snapshot = StorageFootprint::measure(&ctx.env, &contract_id);

// Bytes, and the Stroops to hold them for another 518,400 ledgers.
let bytes = snapshot.total_bytes();
let stroops = snapshot.projected_rent(DEFAULT_PROJECTION_LEDGERS);
```

The arithmetic is the network's own: each entry is priced through
`soroban_env_host::fees::compute_rent_fee`, the function core calls, so a
projection agrees with what the protocol charges rather than with a restatement
of the formula here. A projection is the cost of one TTL bump from zero, which
means it is rent *plus* the per-entry write *plus* the 48-byte `TtlEntry` the
bump itself writes. Doubling the horizon therefore adds rent without adding a
second entry write.

`rent_estimate` breaks the same number up per tier, and `RentParameters` is
where you override the fee set:

```rust
use soroban_testkit_core::{RentParameters, StorageFootprint};

let params = RentParameters::default()
    // Re-price rent per KB for a network holding more state than the target,
    // following core's own curve instead of the floor.
    .with_soroban_state_size(6_000_000_000);
let estimate = StorageFootprint::measure(&ctx.env, &contract_id)
    .rent_estimate(&params, 518_400);

let persistent = estimate.persistent_stroops;
assert!(estimate.total_stroops() >= persistent);
```

`to_markdown` renders the entries and the projection as one table, for a PR
comment or a docs page:

```rust
print!("{}", snapshot.to_markdown());
```

```markdown
### Entries

| Key | Bytes |
|-----|--------:|
| `instance:admin` | 24 |
| `instance:paused` | 24 |
| `persistent:count` | 80 |
| `persistent:owner` | 80 |
| `temporary:nonce` | 80 |

| Tier | Entries | Bytes | Rent for 518400 ledgers |
|------|--------:|------:|----------------------:|
| instance | 2 | 48 | 40047 |
| persistent | 2 | 160 | 106762 |
| temporary | 1 | 80 | 36714 |
| **Total** | **5** | **288** | **183523** |

<sub>Sizes are XDR-encoded bytes. Rent is priced by `soroban-env-host`'s
`compute_rent_fee` at the protocol 23 fee parameters, entry write and
`TtlEntry` included.</sub>
```

Two things about those numbers are worth reading carefully. The tier rows are
not three copies of the same arithmetic: persistent and temporary keys are each
their own ledger entry, so each pays its own entry write, while every key in
instance storage lives inside one `ContractData` entry and is priced as that
single map. That is why two 24-byte instance keys rent for less than one 80-byte
persistent key. And because the snapshot records the key-plus-value map entries
of instance storage rather than the envelope around them, the instance column is
a floor. `StorageFootprint::to_markdown` does the same for every contract in the
artifact, one section each, under a `## \`label\`` heading.

## Storage fragmentation analyzer

Storing data across dozens of small scalar entries inflates the ledger footprint due to repeated `LedgerKey` overhead and redundant key serialization. `StorageFragmentationAnalyzer` inspects contract storage across `instance`, `persistent`, and `temporary` tiers to measure key-to-value byte ratios, identify fragmented entries sharing common prefixes, and calculate projected byte and Stroop rent savings:

```rust
use soroban_testkit_core::{StorageFragmentationAnalyzer, StorageTier};

let analyzer = StorageFragmentationAnalyzer::new()
    .with_overhead_threshold(0.40)      // Flag entries where key bytes >= 40% of total
    .with_stroop_rate_per_byte(100);    // Rent rate assumption for Stroop savings

let report = analyzer.analyze(&env, &contract_id);

println!("Overall overhead ratio: {:.1}%", report.overall_overhead_ratio * 100.0);
println!("Potential byte savings: {} B", report.total_projected_byte_savings());
println!("Projected Stroop savings: {} Stroops", report.total_projected_stroop_savings());

// Render an optimization report as Markdown:
let markdown = report.to_markdown();
```

## Call-stack depth auditing

Deep contract composition has a hard ceiling: the host refuses one more nested
call once the stack is full. `StackDepthAuditor` reconstructs the chain that
ran, reports its high-water depth, and checks it against a cap a test controls.

```rust
use soroban_testkit_core::StackDepthAuditor;

client.deposit(&user, &1_000); // proxy -> vault -> token

let auditor = StackDepthAuditor::from_env(&env);
println!("{}", auditor.trace());
// call trace: 3 frame(s), max depth 3, cap 100, warning threshold 90
//   depth=1 contract=CDEPOSIT... function=deposit
//   depth=2 contract=CVAULT... function=transfer_in
//   depth=3 contract=CTOKEN... function=transfer

assert_eq!(auditor.max_depth(), 3);
assert!(!auditor.warning_triggered());
auditor.assert_within(); // panics with the full trace if the cap was passed
```

### Where the depth comes from

The pinned host does not expose a public per-frame hook. `Host::set_trace_hook`
is crate-private, and the public `Host::set_invocation_hook` fires only when a
top-level invocation starts and finishes, not for each nested call. What the
host does expose is its diagnostic event stream: `fn_call` is emitted before a
frame opens and carries the called contract id and function name, and
`fn_return` is emitted when that call returns. `StackDepthAuditor` reads those
events through `Env::host().get_diagnostic_events()` and nests them in order,
which makes the reconstruction exact for successful cross-contract chains.

For a harness or an instrumented test contract, `enter(contract, function)`
and `exit()` record the same frames by hand:

```rust
use soroban_testkit_core::StackDepthAuditor;

let mut auditor = StackDepthAuditor::new();
auditor.enter("CPROXY", "forward").enter("CIMPL", "credit");
assert_eq!(auditor.max_depth(), 2);
auditor.exit();
```

### The host limit

The cap defaults to the host's own limit, not a number Testkit guessed.
`soroban-env-host` 28.0.2 defines `DEFAULT_HOST_DEPTH_LIMIT = 100` in
`src/budget/limits.rs`, and `Host::with_frame` in `src/host/frame.rs` returns
`Context/ExceededLimit` when the context stack has already reached it. A test
environment's top-level client call starts with an empty context stack, so
contract frames 1 through 100 are accepted and the 101st nested call is the one
that fails. An entry point reached through `Host::invoke_function` pushes a
host-function frame first, which leaves room for 99 contract frames. The
constant is re-exported as `HOST_CALL_STACK_LIMIT`.

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">A warning is not a failure</span>
  <p><code>warning_triggered()</code> and <code>warnings()</code> report frames at or beyond the warning threshold — 90% of the cap by default, set with <code>with_warning_threshold</code>. Only <code>breach()</code> and <code>assert_within()</code> treat a depth as a failure.</p>
</div>

### Lowering the cap to test recovery

Running 100 nested calls to reach the real limit is slow and awkward. A test
can check the same contract against a shallower ceiling instead:

```rust
use soroban_testkit_core::StackDepthAuditor;

let auditor = StackDepthAuditor::from_env(&env).with_lowered_cap(4);
if auditor.breach().is_some() {
    // The contract's fallback path: split the call, or return a partial result.
}

// Or check a recorded trace against a hypothetical cap without changing it.
let would_breach = auditor.simulate_lowered_cap(2);
assert!(would_breach.is_some());
```

`with_lowered_cap` reduces the auditor's cap and `simulate_lowered_cap`
answers what a different cap would have said about the same trace; neither
changes the host. `StackDepthBreach` carries `max_depth`, `cap` and the
rendered trace, so a panic message lists every frame rather than only the
number that was too large.

<div class="tk-callout">
  <span class="tk-callout__title">What the trace cannot show</span>
  <p><code>Env::default()</code> enables diagnostics, which is what makes the reconstruction possible. A call that traps emits no <code>fn_return</code>, so its frame is recorded but never paired with a return; the high-water depth is still correct. The host also refuses contract re-entry before it emits a nested <code>fn_call</code>, so a refused recursive call leaves no extra frame. Where diagnostics are unavailable, use <code>enter</code>/<code>exit</code> from an instrumented harness.</p>
</div>

## SDK v29 compatibility auditor

As protocol standards move from Protocol 21/22 toward Protocol 23 and SDK v29, legacy host functions and untyped APIs are deprecated. `CompatibilityAuditor` monitors contract invocations, events, and host errors to identify deprecated calls early, outputting a compatibility scorecard with concrete remediation instructions:

```rust
use soroban_testkit_core::{CompatibilityAuditor, CompatibilityGrade};

let mut auditor = CompatibilityAuditor::new()
    .with_target_protocol(22); // Target Protocol 22 / SDK v29

// Audit contract interactions:
let scorecard = auditor.audit_contract(&env, &contract_id);

println!("Readiness grade: {}", scorecard.grade.as_str());
assert!(scorecard.is_ready_for_upgrade, "Contract contains critical obsolete calls!");

// Render a CI-ready Markdown report:
let markdown_report = scorecard.to_markdown();
```

## Reentrancy guard and state access ordering auditor

DeFi contracts using external callbacks, token transfers, or multi-contract routing
must guarantee that reentrancy guard flags cannot be bypassed via indirect re-entry
paths. Static and dynamic verification of state access order ensures protocol safety
under Check-Effects-Interactions (CEI) patterns.

[`ReentrancyGuardAuditor`] monitors lock flag states and cross-contract invocations,
verifying that:
1. Reentrancy locks are set (`LockState::Locked`) *before* any external invocation is performed.
2. Locks are initialized and never accessed while uninitialized.
3. Locks are not prematurely released before external calls finish.
4. Locks are properly reset (`LockState::Unlocked`) upon frame exit.

### Static trace verification

Harnesses and unit tests can verify execution sequences directly:

```rust
use soroban_testkit_core::{LockState, ReentrancyGuardAuditor};

let mut auditor = ReentrancyGuardAuditor::new();
auditor
    .enter_frame("CVAULT", "withdraw")
    .set_lock("CVAULT", LockState::Locked)
    .external_call("CVAULT", "CTOKEN", "transfer")
    .enter_frame("CTOKEN", "transfer")
    .exit_frame("CTOKEN", "transfer")
    .set_lock("CVAULT", LockState::Unlocked)
    .exit_frame("CVAULT", "withdraw");

let report = auditor.audit();
assert!(report.is_safe());
```

### Detecting flawed reentrancy guard implementations

When a contract fails to initialize its lock before calling an external callback or releases its guard prematurely, `assert_reentrancy_safe()` flags the vulnerability and renders a state access ordering timeline:

```rust,should_panic
use soroban_testkit_core::ReentrancyGuardAuditor;

let mut auditor = ReentrancyGuardAuditor::new();
auditor
    .enter_frame("CVAULT", "withdraw")
    // Flaw: External call performed while lock is uninitialized!
    .external_call("CVAULT", "CTOKEN", "transfer")
    .exit_frame("CVAULT", "withdraw");

// Panics with visual state access ordering timeline:
auditor.assert_reentrancy_safe();
```

### Visual state access ordering timeline

On assertion failure or via `report.render_timeline()`, the auditor outputs an ASCII execution timeline detailing every step, depth, contract, action, lock state, and violation diagnostic:

```text
====================================================================================================
                        STATE ACCESS ORDERING & REENTRANCY TIMELINE
====================================================================================================
Step | Depth | Contract   | Action                                | Lock State    | Status / Notes
-----+-------+------------+---------------------------------------+---------------+-----------------
1    | 1     | CVAULT     | Enter: withdraw                       | Uninitialized | OK
2    | 1     | CVAULT     | ExternalCall -> CTOKEN::transfer      | Uninitialized | [VIOLATION] External call made while lock flag is Uninitialized on 'CVAULT'
3    | 1     | CVAULT     | Exit: withdraw                        | Uninitialized | OK
----------------------------------------------------------------------------------------------------
Result: FAILED — 1 reentrancy violation(s) detected:
  1. [Step 2] External call from 'CVAULT' to 'CTOKEN::transfer' while lock is Uninitialized
====================================================================================================
```

### Dynamic environment observation

The auditor can also observe contract invocations dynamically from the test environment:

```rust
use soroban_testkit_core::ReentrancyGuardAuditor;
# use soroban_sdk::Env;

# fn audit_env(env: &Env) {
let auditor = ReentrancyGuardAuditor::from_env(env);
let report = auditor.audit();
if report.has_violations() {
    println!("{}", report.render_timeline());
}
# }
```

<hr class="tk-divider" />

<p class="tk-muted">Next: <a href="./assert.html">soroban-testkit-assert</a> — turn these measurements into readable assertions.</p>
