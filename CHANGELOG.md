# Soroban Testkit Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-10-09

The first release on crates.io.

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
- Installation moved from a Git tag to a crates.io version requirement, with the tagged Git pin kept as the build-from-source option.
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
