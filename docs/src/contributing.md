# Contributing

Contributions of every size are welcome. The repository guide — [CONTRIBUTING.md](https://github.com/stellar-crucible/soroban-testkit/blob/main/CONTRIBUTING.md) — is the authoritative version; this page summarises the flow and the rewards.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Toolchain</span><span class="tk-spec__value">Rust 1.91+ (stable)</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Checks</span><span class="tk-spec__value">fmt · clippy · test · docs · cargo deny</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Review</span><span class="tk-spec__value">two-way, within 14 days</span></div>
</div>

## Contribution flow

<ol class="tk-steps">
  <li>
    <h4>Fork and clone</h4>
    <p>Create your fork of <code>stellar-crucible/soroban-testkit</code>, then <code>cargo test --workspace --locked</code> to confirm a green baseline (130 tests).</p>
  </li>
  <li>
    <h4>Claim an issue</h4>
    <p>Every funded task carries both <code>Stellar Wave</code> and a <code>complexity:*</code> label. Comment on the issue before starting so two contributors do not collide — Wave issues are first-come, first-served.</p>
  </li>
  <li>
    <h4>Branch, commit, check</h4>
    <p>Run <code>cargo fmt --all --check</code> and <code>cargo clippy --workspace --all-targets --locked -- -D warnings</code> locally. <code>main</code> is protected: <code>check</code>, <code>Docs build</code> and <code>Supply chain audit</code> must all be green before a pull request can merge.</p>
  </li>
  <li>
    <h4>Open a pull request</h4>
    <p>Reference the issue, describe what changed and why, and include the test evidence for behaviour changes.</p>
  </li>
</ol>

## Stellar Wave rewards

This project participates in the [Stellar Wave](https://www.drips.network/wave/stellar) program on [Drips Network](https://www.drips.network). Every issue carries a complexity label, and rewards land when the pull request merges.

<div class="tk-grid tk-grid--3">
  <div class="tk-card">
    <span class="tk-card__kicker">complexity:trivial</span>
    <h3 class="tk-card__title">100 points</h3>
    <p class="tk-card__body">Docs, small type fixes, one-line helpers. Expected within a day or two.</p>
  </div>
  <div class="tk-card">
    <span class="tk-card__kicker">complexity:medium</span>
    <h3 class="tk-card__title">150 points</h3>
    <p class="tk-card__body">A new matcher, a generator family, or an API that touches one crate.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">complexity:high</span>
    <h3 class="tk-card__title">200 points</h3>
    <p class="tk-card__body">Cross-crate features, storage enumeration, or anything needing design discussion first.</p>
  </div>
</div>

<div class="tk-callout">
  <span class="tk-callout__title">Unresolved issues roll over</span>
  <p>If an issue is not merged inside the monthly cycle it carries over with its label intact — picking it up late still pays.</p>
</div>

<div class="tk-cta">
  <p>Ready to start? The tracker lists every open task with its complexity label and acceptance criteria.</p>
  <a class="tk-btn tk-btn--primary" href="https://github.com/stellar-crucible/soroban-testkit/issues">Browse issues</a>
</div>
