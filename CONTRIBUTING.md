# Contributing to Soroban Testkit

Thank you for your interest in contributing! This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program, and we welcome contributors of all experience levels.

## Getting Started

1. Fork and clone the repository
2. Install Rust 1.91+ via [rustup](https://rustup.rs/)
3. Run `cargo test --workspace --locked` to verify everything works — 130 tests should pass, plus 2 ignored baseline jobs that only CI enforces
4. Pick an issue labeled `Stellar Wave` plus a complexity level (`complexity:trivial`, `complexity:medium`, or `complexity:high`); comment on it before starting so two people don't collide

## Development

```bash
cargo test --workspace --locked          # all crates + examples/counter
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check                  # CI runs the check, not the write
mdbook build docs                        # documentation site must build clean
cargo deny check all                     # supply-chain gate over the committed Cargo.lock
```

### Budget baselines

[`examples/counter/budget.json`](examples/counter/budget.json) records what the counter's hot paths cost, and the `Budget baseline` workflow holds every pull request to it on one pinned runner. The two tests that record and check it are `#[ignore]`d, so a local `cargo test` never fails over a machine's numbers. When a change is *meant* to move a cost, run that workflow with **Record** checked and commit the file it uploads — the pull request then shows the number moving instead of a red check being retried.

## Issue Labels

| Label | Description | Points |
|-------|-------------|--------|
| `complexity:trivial` | Small, well-scoped task | 100 |
| `complexity:medium` | Moderate feature or improvement | 150 |
| `complexity:high` | Significant feature or complex task | 200 |
| `Stellar Wave` | Funded through the Drips Stellar Wave program | — |

Additional labels: `good first issue`, `bug`, `enhancement`, `documentation`, `help wanted`.

## Pull Request Process

1. Create a branch from `main` with a descriptive name
2. Implement your changes with tests — every new API needs a test that fails without it
3. Ensure `cargo test`, `cargo clippy` and `cargo fmt --check` all pass locally with `RUSTFLAGS="-D warnings"`
4. If you changed behaviour users can see, update the matching page under `docs/src/` in the same PR
5. Submit a PR using the template; `main` is protected, so the `check`, `Docs build` and `Supply chain audit` jobs must be green before it can merge. A PR that touches the example contract or the budget code also runs `Budget baseline`, which posts the measured costs as a comment.
6. A maintainer will review and merge

## Releasing

A release is a version bump, a crates.io publish in dependency order, and a tag. Only a maintainer does it, and only from a commit whose CI is green.

```bash
# 1. Bump [workspace.package] version in Cargo.toml, and the `version = "…"`
#    on the internal soroban-testkit-core path deps in the other manifests.
cargo test --workspace            # refreshes Cargo.lock for the new versions
cargo test --workspace --locked   # then prove the committed lock still builds
mdbook build docs
cargo deny check all

# 2. Publish. `-core` first — the dependants cannot package until it is on the index.
cargo publish -p soroban-testkit-core
cargo publish -p soroban-testkit-assert
cargo publish -p soroban-testkit-fixtures
cargo publish -p soroban-testkit-generators

# 3. Tag and release from the exact commit that shipped.
git tag -a vX.Y.Z -m "vX.Y.Z" && git push origin vX.Y.Z
```

`examples/counter` carries `publish = false` — it is a worked example, not a distribution.

Move the `Unreleased` CHANGELOG entries into a `[X.Y.Z]` section with the date in the same commit as the version bump, and add the section's link reference to the tag URL. Update `Installation` and the roadmap's `Current` row to the new version, since both quote a version requirement.

## Code Style

- Follow standard Rust conventions enforced by `rustfmt` and `clippy`
- Write tests for new functionality
- Keep functions focused — prefer small, composable utilities
- Use meaningful variable names; avoid abbreviations

## Questions?

Open an issue or reach out on the [Stellar Developer Discord](https://discord.gg/stellardev).
