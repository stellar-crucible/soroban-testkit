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

- [ ] This PR resolves an issue labeled with a `complexity:` tag
- [ ] I raised the issue first and a maintainer approved the scope

## Acceptance criteria check

Re-read the linked issue and confirm each box in its Acceptance Criteria section:

- [ ] All acceptance criteria are met
- [ ] Example usage from the issue compiles (if the issue defines one)
- [ ] Public API additions are documented with doc comments
- [ ] A test proves the new behaviour, and it fails without the change

## Verification

Run these locally before requesting review — CI treats warnings as errors:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo test --workspace
```

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets` passes with `-D warnings`
- [ ] `cargo test --workspace` passes
- [ ] Docs build (`mdbook build docs`) passes, if docs changed

## Reviews

Two-way review is required: the contributor reviews the maintainer's work in the thread, and a maintainer reviews the PR. Both sides have 14 days; unresolved issues roll into the next Wave cycle.

## Breaking changes

- [ ] No breaking changes, or the break is described below with a migration path

