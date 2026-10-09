# Contributing to Soroban Testkit

Thank you for your interest in contributing! This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program, and we welcome contributors of all experience levels.

## Getting Started

1. Fork and clone the repository
2. Install Rust 1.91+ via [rustup](https://rustup.rs/)
3. Run `cargo test` to verify everything works
4. Pick an issue labeled with a complexity level (`complexity:trivial`, `complexity:medium`, or `complexity:high`)

## Development

```bash
# Run all tests
cargo test --workspace

# Run clippy
cargo clippy --workspace -- -D warnings

# Format code
cargo fmt --all
```

## Issue Labels

| Label | Description | Points |
|-------|-------------|--------|
| `complexity:trivial` | Small, well-scoped task | 100 |
| `complexity:medium` | Moderate feature or improvement | 150 |
| `complexity:high` | Significant feature or complex task | 200 |

Additional labels: `good first issue`, `bug`, `enhancement`, `documentation`, `help wanted`.

## Pull Request Process

1. Create a branch from `main` with a descriptive name
2. Implement your changes with tests
3. Ensure `cargo test`, `cargo clippy`, and `cargo fmt` all pass
4. Submit a PR with a clear description of what changed and why
5. A maintainer will review and merge

## Code Style

- Follow standard Rust conventions enforced by `rustfmt` and `clippy`
- Write tests for new functionality
- Keep functions focused — prefer small, composable utilities
- Use meaningful variable names; avoid abbreviations

## Questions?

Open an issue or reach out on the [Stellar Developer Discord](https://discord.gg/stellardev).
