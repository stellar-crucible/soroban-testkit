# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.4.x   | :white_check_mark: |
| < 0.4   | :x:                |

Only the latest published minor is supported. This crate is pre-1.0, so a minor bump may change public API; security fixes land in a new minor rather than being back-ported to an older one.

## Reporting a Vulnerability

If you discover a security vulnerability in Soroban Testkit, please report it responsibly.

**Do not open a public GitHub issue for security vulnerabilities.**

Instead, please email the maintainers directly or use GitHub's private vulnerability reporting feature on this repository.

We aim to acknowledge reports within 48 hours and provide an initial assessment within one week.

## What runs on every pull request

`main` is protected. Six required checks have to report before a pull request can merge. Four of them are listed below; the other two, `check` and `Docs build`, are ordinary build checks, and `Malware guard` is a `needs:` dependency of both, so they do not start until it has passed.

| Check | Workflow | What it does |
| ----- | -------- | ------------ |
| `Malware guard` | `ci.yml` | Compares the diff against the code-execution surface this repository has today, and fails on anything new. Runs before anything compiles. |
| `Secret scan` | `security.yml` | Scans the lines the diff adds for credential shapes. |
| `CI integrity` | `security.yml` | Audits the pipeline itself: triggers, permissions, secrets, event-field interpolation into shell, and action provenance. |
| `Supply chain audit` | `ci.yml` | `cargo-deny check all` — RustSec advisories, yanked and unmaintained crates, licenses, duplicate versions, source provenance. |

`Malware guard` is a `needs:` dependency of `check`, `Docs build` and `Supply chain audit`. That ordering is the point: `cargo clippy` and `cargo test` both compile and execute any `build.rs` in the diff, so a pipeline that builds first and inspects afterwards only ever inspects the aftermath. Nothing in this repository compiles a pull request's Rust until the guard has passed.

The guard fails on a new `build.rs` or `[build-dependencies]`, on `.cargo/config.toml`, on `.gitattributes` filter drivers, on submodules, on vendored trees, on binary files and symlinks, on dependencies taken from a git source or redirected through `[patch]`, on crates that reach the network, spawn a process or call into C, and in Rust source on `unsafe`, `process::Command`, `std::net`, `mem::transmute`, `include_bytes!`, `#[no_mangle]` and `extern "C"`. It also fails on edits to `.github/workflows`, `.github/scripts`, `CODEOWNERS`, `dependabot.yml` or `deny.toml`. The baseline for every one of those is zero: this repository has no build script, no `unsafe`, no process or network call and no stored secrets today, which is what makes the rules enforceable rather than aspirational.

Two properties are worth stating because they are the ones a determined pull request would attack:

- **The inspectors run from the base branch.** Each job fetches its own script from the base branch tip and executes that copy, not the one in the change under inspection. A pull request that could edit its own inspector would only have to delete the rule it is about to trip. The exception is the pull request that first introduces a script, where the base branch has nothing to fetch; that pull request is a maintainer's and carries the override label.
- **A required check that never reports blocks the merge.** Deleting `security.yml` or the guard job does not remove the check, it removes the report, and an absent required check leaves the pull request unmergeable. The gate cannot be deleted by the thing it gates.

### The override

`security-override` is a label only a maintainer can apply. It downgrades findings to recorded output and lets the pull request through, so the finding stays visible on the run rather than being quietly edited away. It exists because the rules are strict by design and a legitimate change does sometimes add a build script or edit CI. If your pull request needs it, say why on the pull request. Do not obfuscate the line the guard caught to avoid asking — the label leaves a record, and a worked-around check leaves nothing.

### What this is not

The guard is a diff inspector, not a sandbox and not an antivirus. It is pattern-based, so it catches the shapes above and does not catch a payload that avoids them; a determined author who knows the rules can write Rust that exfiltrates without using any pattern on the list. What that author cannot do is get it merged past a required check that a maintainer has to look at, or get it published, since publishing is manual and version-pinned.

Two known gaps, stated rather than papered over:

- **Third-party actions are not pinned to commit SHAs.** `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2` and `EmbarkStudios/cargo-deny-action@v2` are mutable tag and branch refs. Two things hold the list closed: `CI integrity` allowlists exactly those three and requires anything else to be either first-party or a 40-character SHA, and the repository's Actions settings allow only GitHub-owned actions plus those same three patterns, so a workflow the audit somehow missed would still be refused at runtime. Both are edited only through files the guard protects. What neither addresses is that the three already there can change upstream without a diff in this repository. Pinning them is the obvious next change.
- **The gate inspects the change, not the dependency tree's behaviour.** A malicious version of a legitimate crate is `cargo-deny`'s and the registry's problem, not the guard's. It has no checksum to compare against because this repository does not vendor anything.

GitHub secret scanning and push protection are enabled on the repository. The default token permission for a workflow is read-only, workflows cannot approve pull request reviews, and the runner for a pull request from a fork gets no secrets — there are none stored to give it.

## Scope

This security policy covers:
- The testkit crates and their dependencies
- CI/CD workflows
- Documentation site

Out of scope:
- Vulnerabilities in the Soroban SDK itself (report to Stellar)
- Issues in downstream projects using testkit
