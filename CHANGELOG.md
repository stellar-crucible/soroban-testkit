# Soroban Testkit Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **Installation leads with the registry.** `soroban-testkit-core`, `-assert`, `-fixtures` and `-generators` are published on [crates.io](https://crates.io) at `0.3.0`, so the README, the installation page and the property-testing guide now give a version requirement first and the tagged Git pin as the alternative for anyone who wants to pin the source commit. Checked from a clean project whose only access to the toolkit is the registry: it resolves, compiles and its tests pass ([#34](https://github.com/stellar-crucible/soroban-testkit/issues/34))

## [0.3.0] - 2026-10-09

Everything that landed since `v0.2.0`, each of it through the protected-branch flow: budget regression detection with a committed baseline that CI enforces, ledger time helpers, context reset, assertions on what an event carried, and an event log that spans several invocations. This is the version the registry carries — `soroban-testkit-core`, `soroban-testkit-assert`, `soroban-testkit-fixtures` and `soroban-testkit-generators` `0.3.0` are published on [crates.io](https://crates.io), and `v0.1.x`/`v0.2.0` were never uploaded, so a version requirement below `0.3.0` does not resolve.

### Added
- `soroban-testkit-assert`: `EventLog` aggregates events across invocations, so one assertion can speak about a sequence of calls instead of only the last one. `collect()` after each call the test means to judge, and `matcher()` hands the gathered set to `EventMatcher` — `from_contract`, `with_topic`, `assert_emitted`, `assert_count`, `assert_not_emitted`, `assert_none_match` and `assert_data_matches` all read it ([#19](https://github.com/stellar-crucible/soroban-testkit/issues/19))
- `EventLog::topics()` reads the sequence back as one entry per collected event, so a test can assert the order calls published in; `events()`, `len()` and `is_empty()` cover what the filters cannot express. Collection stays explicit because SDK v28 gives a test no per-invocation hook — a call nobody collected from adds nothing, and a silent call adds nothing either, which is what lets `assert_not_emitted()` mean "nowhere in this test"
- `EventMatcher::new(&env)` keeps its meaning: it still reads `env.events().all()`, the most recent invocation. The contract filter now compares the emitter's contract id itself rather than calling the SDK's `filter_by_contract`, which panics on an account address; an account filters to nothing instead
- 10 more tests, covering the aggregated count against the single-call count, a call left out of the log, a silent call, the contract and topic filters over the sequence, `topics()` ordering, payload assertions that reach every collected call, and the two messages an aggregated failure prints — plus one in `examples/counter` that collects three calls and asserts both the count over the sequence and the first call's `new_count`
- `soroban-testkit-assert`: `EventMatcher::assert_data_matches` asserts on what an event carried. The predicate receives an `EventData` view: `deserialize::<T>()` for a payload of a known type (a number, an `Address`, a `Vec`/`Map`, a `contracttype` struct), `field("name")` for one entry of the map a `contractevent` struct publishes, and `raw()` for the `ScVal` itself ([#18](https://github.com/stellar-crucible/soroban-testkit/issues/18))
- A failed data assertion prints the payloads it rejected rather than only that the predicate was false: scalars by value, maps and vectors by entry, addresses as the strkey a test recognises, and anything rarer in its XDR debug form. Nothing in scope reads differently from data that did not match.
- 12 more tests, one per payload shape a contract publishes — `u32`, `i128`, a flag, a symbol, a vector, a map, a `contracttype` struct deserialized whole and field by field — plus the three messages a failure produces. `examples/counter` now asserts on the `caller` and `new_count` of its `Incremented` event instead of only proving the event fired.
- `soroban-testkit-core`: `BudgetGuard` bounds one named call with an absolute CPU and memory ceiling and a growth allowance measured against a recorded cost, and `run()` judges the metering the invocation itself left behind ([#3](https://github.com/stellar-crucible/soroban-testkit/issues/3))
- `soroban-testkit-core`: `BudgetSnapshot::last_invocation` reads `env.cost_estimate().resources()`, the resources the most recent top-level contract invocation metered, and `BudgetRead` is implemented for `InvocationResources` so any of those readings capture the same way
- `soroban-testkit-core`: `budget_guard!` for the single call site, and `BudgetBaseline` — recorded costs as a JSON file, `{ "version": 1, "cases": { "increment": { "cpu_insns": 32669, "mem_bytes": 5252 } } }`, loaded and saved by path so CI can commit the numbers next to the suite
- A guard reports **every** limit one cost breaks, ceilings before growth, each as one `key=value` line (`BUDGET kind=growth case=transfer metric=cpu_insns actual=1500000 limit=1100000 baseline=1000000 tolerance_percent=10`) so a CI step can grep or trend them without parsing a panic backtrace
- 27 more tests, including four that drive the macro and the guard through a registered contract, five that pin the per-invocation metering itself — a repeated read measures identical, a write measures more than a read of the same entry, `capture()` over resources is `last_invocation()`, and `run()` hands back the value of the call it measured — and three in `examples/counter` that record one real increment and then accept that same call against the baseline it recorded itself
- `examples/counter` now carries a committed budget baseline: `budget.json` holds the cost of `get` and `increment` as one steady-state invocation each, `budget_baseline_records_current_costs` rewrites the file, and `budget_baseline_rejects_drifted_costs` fails with one parseable line per metric when a case grows past the tolerance ([#14](https://github.com/stellar-crucible/soroban-testkit/issues/14))
- A `Budget baseline` workflow runs that check on one pinned `ubuntu-latest` runner on every pull request and on `main`, posts the measured table as a pull request comment, and takes `tolerance`, `fail_on_drift` and `record` on a manual run — record mode rewrites and uploads `budget.json` from the runner that enforces it, so a re-baselined number is measured where it will be judged
- Both budget jobs are `#[ignore]`d. The readings are reproducible — the file recorded on Windows measured identical on `ubuntu-latest`, 0.00% drift on every metric — but they belong to the pinned SDK rather than to any one machine, and the recording test rewrites a file in the repository, so neither belongs in an ordinary `cargo test` run
- `soroban-testkit-fixtures`: `TestContext::reset` swaps in a fresh env — ledger, events, authorizations and contract storage all cleared — while carrying the admin and every user across, so a multi-phase test stops rebuilding its fixture to get a clean chain ([#17](https://github.com/stellar-crucible/soroban-testkit/issues/17))
- `soroban-testkit-fixtures`: `TestContext::reset_full` for the phase that wants new identities as well as a new chain
- 9 more tests, including one proving a carried address still authorizes a contract call in the new env, one proving a contract registered before the reset holds nothing after it, and one pinning the SDK address-counter behaviour `reset_full` works around
- `soroban-testkit-fixtures`: `TestContext::advance_time`, `set_timestamp` and `advance_ledger`, with `timestamp()` and `sequence()` getters, so a time-dependent test no longer hand-builds a `LedgerInfo` ([#12](https://github.com/stellar-crucible/soroban-testkit/issues/12))
- 11 more tests, including two that read the moved ledger back through a registered contract to prove the change reaches the host rather than a cached field

### Changed
- `soroban-testkit-core` now depends on `serde` and `serde_json`, which is what the baseline file is. Both were already resolved in `Cargo.lock` through `soroban-ledger-snapshot`, so no new crate enters the graph.
- `TestContext` gained a public `mock_auths` field, because an `Env` cannot report whether it mocks authorizations and `reset` has to reproduce the policy rather than invent one. The struct now has four fields, so a `TestContext { env, admin, users }` literal no longer compiles — `new`, `with_env` and `TestContextBuilder` are the ways in. A context wrapped around a hand-built unmocked env should set `ctx.mock_auths = false`.
- The two ledger helpers each move one axis. `advance_time` leaves the sequence where it was and `advance_ledger` leaves the clock where it was, so a test that only wants entries to expire does not silently move time as well. Both saturate at `u64::MAX` and `u32::MAX` instead of wrapping.
- **Installation says what resolves today.** The four crates are named, described and versioned for a crates.io release, and the registry does not list them yet — so the README and the installation page now lead with the `v0.2.0` Git pin and present the version requirement as the form to switch to, and the version badge is gone until there is a version to badge. A reader who copy-pasted `soroban-testkit-core = "0.2.0"` got a resolution error, which is a worse first impression than an honest sentence.

## [0.2.0] - 2026-10-09

The release that made the crates publishable: final names, per-crate metadata, and the storage, error and assertion work below. Consume it from the `v0.2.0` tag until the registry carries these names.

### Added
- `soroban-testkit-core`: `StorageSnapshot::capture` enumerates a contract's live instance, persistent and temporary storage entries from the ledger snapshot, replacing the `inspect_storage` stub ([#4](https://github.com/stellar-crucible/soroban-testkit/issues/4))
- `soroban-testkit-core`: `StorageSnapshot::diff` reports added, removed and rewritten keys, with `assert_unchanged`, `assert_entry_added` and `assert_entry_removed` on top of it
- `soroban-testkit-core`: `StorageEntry::qualified` renders a key as `tier:key`, and `StorageSnapshot::in_tier` filters by durability
- `soroban-testkit-core`: `DecodedError::from_error` names the host category and protocol meaning behind a packed error code, covering all ten categories and all ten standard codes ([#5](https://github.com/stellar-crucible/soroban-testkit/issues/5))
- `soroban-testkit-core`: `ErrorRegistry` maps a contract's own `#[contracterror]` codes to words, and `unwrap_decoded` / `unwrap_decoded_with` unwrap a v28 `try_*` client call while panicking with the decoded sentence
- `soroban-testkit-assert`: `EventMatcher::assert_not_emitted` proves the latest invocation published nothing in scope, honouring the contract and topic filters ([#11](https://github.com/stellar-crucible/soroban-testkit/issues/11))
- `soroban-testkit-assert`: `EventMatcher::assert_none_match` takes a predicate over the raw `ContractEvent` for what the topic filter cannot express
- Per-crate READMEs, plus `homepage` and `readme` in every published manifest
- 36 tests added since v0.1.1 — the suite is now 81, covering tier enumeration and rendering, every diff outcome, every error category and code, the registry, the failure messages of both negative assertions, and a real contract call decoded through its generated client

### Changed
- **Package names.** The crates publish as `soroban-testkit-core`, `soroban-testkit-assert`, `soroban-testkit-fixtures` and `soroban-testkit-generators`. `testkit-core` was already taken on crates.io by an unrelated crate published in March 2025, and a half-renamed family would be worse to consume than a consistent one. Rust paths follow (`soroban_testkit_core::storage`), as do the workspace directories.
- `DecodedError` gained a `category` field, so constructing one by hand now names the category. `from_error`, `from_error_with` and `from_contract_code` are the paths that do not need it.
- Host errors render with their category (`Soroban Error [storage/3]: …`); contract errors keep the bare code (`Soroban Error [12]: …`).
- Installation documented in both forms: the crates.io version requirement these package names will use, and the tagged Git pin that resolves today.
- Workspace directories renamed to match the package names, so a path reference in an issue or a doc reads correctly.

## [0.1.1] - 2026-10-09

The first tag whose full CI pipeline — including the supply-chain gate — is green.

### Added
- `docs/src/guides/counter-example.md`: a worked walkthrough of `examples/counter`, covering registration, event assertions under the SDK v28 latest-invocation scope, budget bounds, and proving authorisation with `without_mock_auths()`
- `Cargo.lock` is now committed and CI runs clippy and tests with `--locked`, so `cargo deny` audits the same dependency graph we build against

### Fixed
- `deny.toml`: dropped the `workspace-skip` key that cargo-deny 0.20 rejects, removed advisory ignores for crates we do not depend on, and stated an explicit version on the internal `testkit-core` path dependencies so they are no longer wildcard requirements

## [0.1.0] - 2026-10-09

First release, published for the Stellar Wave funding program.

### Added
- `testkit-core`: `BudgetSnapshot` capturing CPU and memory, with `BudgetRead` so it reads both `soroban_env_host::budget::Budget` and `env.cost_estimate().budget()`
- `testkit-core`: `DecodedError` for human-readable error messages
- `testkit-core`: `StorageEntry`, `StorageTier` and the `inspect_storage` entry point (enumeration still a stub)
- `testkit-assert`: `EventMatcher` with contract and topic filtering over the latest contract invocation
- `testkit-assert`: `AuthMatcher` for authorization verification
- `testkit-fixtures`: `TestContext`, `TestContext::with_env` and `TestContextBuilder`
- `testkit-generators`: proptest strategies for token amounts, ledger sequences and timestamps
- `examples/counter`: a Soroban contract tested with fixtures, event matchers, budget snapshots and an unmocked-authorization failure
- 46 unit tests across all four crates plus the example
- mdBook documentation site with a custom theme, guides, crate references and a roadmap
- CI: fmt, clippy (`-D warnings`), tests, mdBook build and a `cargo deny` supply-chain gate
- Docs deployment to GitHub Pages
- Repo hygiene: issue templates, PR template, `FUNDING.yml`, issue-form config, `CODEOWNERS`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CONTRIBUTING.md`, `deny.toml`
- Crate manifest metadata: keywords, categories and documentation URLs
- Drips Wave complexity labels (trivial/medium/high) and labelled issues

[0.2.0]: https://github.com/stellar-crucible/soroban-testkit/releases/tag/v0.2.0
[0.1.1]: https://github.com/stellar-crucible/soroban-testkit/releases/tag/v0.1.1
[0.1.0]: https://github.com/stellar-crucible/soroban-testkit/releases/tag/v0.1.0
