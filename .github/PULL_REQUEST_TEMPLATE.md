## Description

What does this pull request change, and why? Link the issue it resolves.

Closes #

## Type of change

- [ ] Bug fix
- [ ] New feature (crate API)
- [ ] Documentation
- [ ] Refactor / internal cleanup
- [ ] CI or tooling

## Stellar Wave

- [ ] This PR resolves an issue labeled `Stellar Wave` with a `complexity:` tag
- [ ] I raised the issue first and a maintainer approved the scope

## Acceptance criteria check

Re-read the linked issue and confirm each box in its Acceptance Criteria section:

- [ ] All acceptance criteria are met
- [ ] Example usage from the issue compiles (if the issue defines one)
- [ ] Public API additions are documented with doc comments
- [ ] The matching page under `docs/src/` is updated in this PR, or does not need updating
- [ ] A test proves the new behaviour, and it fails without the change

## Verification

Run these locally before requesting review — CI treats warnings as errors, and `main` requires the `check`, `Docs build` and `Supply chain audit` jobs to pass:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
mdbook build docs
cargo deny check all
```

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets --locked` passes with `-D warnings`
- [ ] `cargo test --workspace --locked` passes
- [ ] Docs build (`mdbook build docs`) passes
- [ ] `cargo deny check all` passes against the committed `Cargo.lock`

## Reviews

Two-way review is required: the contributor reviews the maintainer's work in the thread, and a maintainer reviews the PR. Both sides have 14 days; unresolved issues roll into the next Wave cycle.

## Breaking changes

- [ ] No breaking changes, or the break is described below with a migration path

