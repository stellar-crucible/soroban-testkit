# Installation

Add only the crates your test suite needs. Every crate lives in this workspace and is consumed straight from Git.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Rust</span><span class="tk-spec__value">1.91 or newer</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Soroban SDK</span><span class="tk-spec__value">28.x</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Edition</span><span class="tk-spec__value">2021</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">License</span><span class="tk-spec__value">Apache-2.0</span></div>
</div>

## Add the dev-dependencies

```toml
[dev-dependencies]
testkit-core = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-assert = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-fixtures = { git = "https://github.com/stellar-crucible/soroban-testkit" }
testkit-generators = { git = "https://github.com/stellar-crucible/soroban-testkit", features = ["proptest"] }
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

### testkit-generators

| Feature | Description |
|---------|-------------|
| `proptest` (default) | Enable proptest strategies |
| `arbitrary` | Enable arbitrary trait implementations |

<div class="tk-callout">
  <span class="tk-callout__title">Versioning</span>
  <p>Testkit is pre-1.0. Pin a tag or revision once your suite is green — <code>cargo update</code> can otherwise pull breaking changes from <code>main</code>.</p>
</div>
