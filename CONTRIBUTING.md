# Contributing to Soroban Testkit

Thank you for your interest in contributing! This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program, and we welcome contributors of all experience levels.

## Getting Started

1. Fork and clone the repository
2. Install Rust 1.91+ via [rustup](https://rustup.rs/)
3. Run `cargo test --workspace --locked` to verify everything works — 46 tests should pass
4. Pick an issue labeled `Stellar Wave` plus a complexity level (`complexity:trivial`, `complexity:medium`, or `complexity:high`); comment on it before starting so two people don't collide

## Development

```bash
cargo test --workspace --locked          # all crates + examples/counter
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check                  # CI runs the check, not the write
mdbook build docs                        # documentation site must build clean
cargo deny check all                     # supply-chain gate over the committed Cargo.lock
```

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
5. Submit a PR using the template; `main` is protected, so the `check`, `Docs build` and `Supply chain audit` jobs must be green before it can merge
6. A maintainer will review and merge

## Code Style

- Follow standard Rust conventions enforced by `rustfmt` and `clippy`
- Write tests for new functionality
- Keep functions focused — prefer small, composable utilities
- Use meaningful variable names; avoid abbreviations

## Questions?

Open an issue or reach out on the [Stellar Developer Discord](https://discord.gg/stellardev).
