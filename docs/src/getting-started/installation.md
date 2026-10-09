# Installation

Add only the crates your test suite needs. All four are published on [crates.io](https://crates.io) as `soroban-testkit-core`, `soroban-testkit-assert`, `soroban-testkit-fixtures` and `soroban-testkit-generators`. If you would rather pin the source, every release is also tagged — a tag points at a commit whose CI (tests, clippy and the `cargo deny` supply-chain gate) is green.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Rust</span><span class="tk-spec__value">1.91 or newer</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Soroban SDK</span><span class="tk-spec__value">28.x</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Edition</span><span class="tk-spec__value">2021</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">License</span><span class="tk-spec__value">Apache-2.0</span></div>
</div>

## Add the dev-dependencies

```toml
[dev-dependencies]
soroban-testkit-core = "0.3.0"
soroban-testkit-assert = "0.3.0"
soroban-testkit-fixtures = "0.3.0"
soroban-testkit-generators = { version = "0.3.0", features = ["proptest"] }
```

The same set pinned to the release tag:

```toml
[dev-dependencies]
soroban-testkit-core = { git = "https://github.com/stellar-crucible/soroban-testkit", tag = "v0.3.0" }
soroban-testkit-assert = { git = "https://github.com/stellar-crucible/soroban-testkit", tag = "v0.3.0" }
soroban-testkit-fixtures = { git = "https://github.com/stellar-crucible/soroban-testkit", tag = "v0.3.0" }
soroban-testkit-generators = { git = "https://github.com/stellar-crucible/soroban-testkit", tag = "v0.3.0", features = ["proptest"] }
```

## Install steps

<ol class="tk-steps">
  <li>
    <h4>Check your toolchain</h4>
    <p>Soroban SDK v28 needs a recent Cargo. Run <code>rustc --version</code> and upgrade with <code>rustup update stable</code> if it reports anything below 1.91.</p>
  </li>
  <li>
    <h4>Pin the SDK your contract already uses</h4>
    <p>Testkit is built against SDK v28. Match the <code>soroban-sdk</code> version in your contract crate so the <code>testutils</code> features resolve to one copy.</p>
  </li>
  <li>
    <h4>Enable <code>testutils</code> in tests</h4>
    <p>Address generation, event inspection and auth recording are gated behind the SDK's <code>testutils</code> feature — the dev-dependency above turns it on for the test profile only.</p>
  </li>
  <li>
    <h4>Verify the wiring</h4>
    <p>Run <code>cargo test</code>. A green compile means the crates are linked; the <a href="./quick-start.html">Quick Start</a> gives you a first test to paste in.</p>
  </li>
</ol>

## Feature flags

### soroban-testkit-generators

| Feature | Description |
|---------|-------------|
| `proptest` (default) | Enable proptest strategies |
| `arbitrary` | Enable arbitrary trait implementations |

<div class="tk-callout">
  <span class="tk-callout__title">Versioning</span>
  <p>Testkit is pre-1.0. A <code>0.x</code> release may break the API, so pin an exact version or a release tag once your suite is green — <code>cargo update</code> can otherwise pull breaking changes from a newer <code>0.x</code>.</p>
</div>
