# soroban-testkit-generators

Soroban-aware property-testing strategies for `proptest`, so generated inputs respect the ranges real contracts accept.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Features</span><span class="tk-spec__value">proptest (default)</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">strategies · errors · multisig</span></div>
</div>

## Available strategies

| Strategy | Type | Range |
|----------|------|-------|
| `token_amount()` | `i128` | 0 to 1e15 |
| `ledger_sequence()` | `u32` | 1 to 10,000,000 |
| `timestamp()` | `u64` | Sep 2020 to May 2033 |
| `address()` | `AddressBytes` | an account or contract address |
| `account_address()` | `AddressBytes` | an ed25519 account (`G...`) |
| `contract_address()` | `AddressBytes` | a contract id (`C...`) |
| `address_of_type(kind)` | `AddressBytes` | the format you ask for |
| `host_error_code_coverage()` | `HostError` | all 10 categories × all 10 codes |
| `host_error()` | `HostError` | alias for `host_error_code_coverage()` |
| `host_error_type()` | `ScErrorType` | every host category |
| `host_error_code()` | `ScErrorCode` | every host code |
| `host_error_of_type(kind)` | `HostError` | the codes of one category |
| `multisig_policy(min, max)` | `MultisigPolicy` | weighted signers and a reachable threshold |
| `valid_signer_subset(policy)` | `Vec<AddressBytes>` | signers whose weight meets the threshold |
| `insufficient_signer_subset(policy)` | `Vec<AddressBytes>` | signers whose weight stays below it |
| `multisig_with_valid_subset(range)` | `(MultisigPolicy, Vec<AddressBytes>)` | a policy and an approving subset of it |
| `multisig_with_insufficient_subset(range)` | `(MultisigPolicy, Vec<AddressBytes>)` | a policy and a failing subset of it |
| `signer_weight()` | `u32` | 1 to 100 |

## Usage with proptest

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn transfer_preserves_total_supply(
        amount in strategies::token_amount(),
        seq in strategies::ledger_sequence(),
    ) {
        // amount is always a valid token balance
        // seq is always a realistic ledger sequence number
    }
}
```

## Addresses

Both Soroban address formats are 32 opaque bytes, so every address strategy
returns an `AddressBytes` that carries those bytes *and* the format they were
generated as. `as_bytes()` borrows the raw value, `into_bytes()` hands over
ownership of the `[u8; 32]`, `address_type()` reads the format tag back, and
`to_address(&env)` turns it into the `Address` a contract call expects.
`ScAddress::from(address_bytes)` gives the XDR form instead, which is what you
want when comparing against a ledger value rather than calling a contract.

```rust
use proptest::prelude::*;
use soroban_sdk::{Address, Env};
use soroban_testkit_generators::strategies::{self, AddressType};

proptest! {
    #[test]
    fn transfer_reaches_the_recipient(recipient in strategies::account_address()) {
        let env = Env::default();
        let recipient: Address = recipient.to_address(&env);

        // recipient is always a valid ed25519 account address
    }
}
```

Filter by format with `address_of_type`, or use the two named shortcuts when a
test only cares about one side of a transfer:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies::{self, AddressType};

proptest! {
    #[test]
    fn a_contract_never_receives_a_signature(
        contract in strategies::address_of_type(AddressType::Contract),
    ) {
        assert_eq!(contract.address_type(), AddressType::Contract);
        assert_eq!(contract.as_bytes().len(), 32);
    }
}
```

<div class="tk-callout">
  <span class="tk-callout__title">No duplicates by construction</span>
  <p>Each generated address carries a process-wide nonce, so two calls to <code>address()</code> in the same test run never produce the same bytes — even if their random prefixes collide. The trade-off is that values depend on generation order, so a failing case should be judged by the bytes proptest prints rather than by replaying a random seed alone.</p>
</div>

## Host error discriminants

Testkit also generates the failures themselves. Soroban reports a host error as
a category ([`ScErrorType`](https://docs.rs/soroban-sdk/latest/soroban_sdk/xdr/enum.ScErrorType.html))
paired with a code ([`ScErrorCode`](https://docs.rs/soroban-sdk/latest/soroban_sdk/xdr/enum.ScErrorCode.html)),
ten of each, so the whole domain is one hundred pairs. `host_error_code_coverage()`
draws from all of them, which is what lets a property test drive an
error-recovery block with failures nobody typed by hand:

```rust
use proptest::prelude::*;
use soroban_sdk::xdr::ScErrorType;
use soroban_testkit_generators::errors::host_error_code_coverage;

enum Recovery {
    Retry,
    Stop,
}

fn recover(error: &soroban_sdk::Error) -> Recovery {
    // A total classification: every category answers, none panics.
    if error.is_type(ScErrorType::Budget) || error.is_type(ScErrorType::Storage) {
        Recovery::Retry
    } else {
        Recovery::Stop
    }
}

proptest! {
    #[test]
    fn recovery_handles_every_host_failure(error in host_error_code_coverage()) {
        let decision = recover(&error.as_error());

        if matches!(error.error_type(), ScErrorType::Budget | ScErrorType::Storage) {
            assert!(matches!(decision, Recovery::Retry));
        } else {
            assert!(matches!(decision, Recovery::Stop));
        }
    }
}
```

`HostError` carries the pair and answers what a test usually needs to name it:
`error_type()` and `code()` return the discriminants, `category()` and
`code_name()` return `"storage"` and `"MissingValue"`, `label()` renders
`storage/3`, `is_contract_error()` separates an application code from a host
failure, and `as_error()` produces the [`Error`](https://docs.rs/soroban-sdk/latest/soroban_sdk/struct.Error.html)
a contract call returns. `host_error_of_type(kind)` narrows a run to one branch
when a category needs its own treatment, and `all_host_errors()` is the full
hundred in table order for a deterministic sweep.

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking goes to the smallest discriminant</span>
  <p>The strategy is two bounded integers — one position in the category table, one in the code table — rather than a <code>prop_oneof!</code> over the pairs. proptest shrinks integers towards zero, and both tables are in XDR discriminant order, so a failing case simplifies to <code>Contract</code>/<code>ArithDomain</code> (<code>contract/0</code>) instead of stopping at whichever branch of the <code>oneof</code> was taken.</p>
</div>

## Multi-signature threshold policies

A multi-sig wallet, a governance council and a threshold escrow share one
shape: weighted signers and a threshold their combined approval must reach.
Hand-building those cases is where threshold tests go wrong — a threshold above
the total weight can never be met, and a "failing" subset that accidentally
clears the threshold makes the negative branch a false positive.

`multisig_policy(min, max)` draws a signer count in the range, a positive
weight for each signer, and then a threshold between one and the total weight.
The policy therefore always has a reachable threshold, the whole signer set
always approves, and the empty set never does. Signers are distinct ed25519
accounts from `account_address()`, so each policy is a set of real addresses a
contract can authenticate.

`valid_signer_subset` and `insufficient_signer_subset` take a policy and return
a strategy over subsets of its signers. Both **repair** the subset they draw
instead of filtering it: an approving subset is topped up until it reaches the
threshold, and a failing one has signers removed until it drops below. That
matters twice over — no case is rejected, so a run says something about every
draw, and shrinking a failing case cannot break the invariant it exists to
hold.

```rust
use proptest::prelude::*;
use soroban_testkit_generators::multisig::{
    multisig_with_insufficient_subset, multisig_with_valid_subset,
};

proptest! {
    #[test]
    fn a_wallet_releases_only_at_the_threshold(
        (policy, approving) in multisig_with_valid_subset(3..10),
    ) {
        assert!(policy.threshold() <= policy.subset_weight(&approving));
        // wallet.verify_approval(&approving) is always true for this input
    }

    #[test]
    fn a_wallet_refuses_a_short_subset(
        (policy, short) in multisig_with_insufficient_subset(3..10),
    ) {
        assert!(policy.subset_weight(&short) < policy.threshold());
    }
}
```

A `MultisigPolicy` exposes `signers()`, `threshold()` and `total_weight()`,
plus `subset_weight(&subset)`, `approves(&subset)` and `addresses()` for
asserting on a subset directly. `Signer` carries `address()` and `weight()`. A
repeated address counts once, so a duplicated signature cannot inflate a
subset over the threshold — the property a real wallet's deduplication check
exists to enforce.

```rust
use soroban_testkit_generators::multisig::{signer_weight, MultisigPolicy, Signer};

// Build a policy by hand when the test names its signers.
let policy = MultisigPolicy::new(
    vec![Signer::new(alice, 2), Signer::new(bob, 1)],
    2,
);
assert!(policy.approves(&[alice]));
assert!(!policy.approves(&[bob]));
```

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking keeps the invariant</span>
  <p>A subset does not come from a <code>prop_filter</code>, so proptest never throws a case away and shrinking cannot produce an invalid one. The approving strategy shrinks towards the smallest prefix that still reaches the threshold; the failing strategy shrinks towards the empty subset, which is below any positive threshold.</p>
</div>

## Custom strategies

Compose the built-ins with proptest combinators:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

fn transfer_args() -> impl Strategy<Value = (i128, i128)> {
    (strategies::token_amount(), strategies::token_amount())
        .prop_filter("sender must have sufficient balance", |(a, b)| a >= b)
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Keep cases bounded</span>
  <p>Soroban tests run on the host and pay for every instruction. Start <code>proptest!</code> cases at the default 256, and shrink ranges before raising the count — <a href="../guides/property-testing.html">Property Testing</a> covers the trade-offs.</p>
</div>

<div class="tk-grid tk-grid--2">
  <div class="tk-card">
    <span class="tk-card__kicker">Feature flags</span>
    <h3 class="tk-card__title">One backend today</h3>
    <p class="tk-card__body"><code>proptest</code> is the default and currently the only backend, and it gates the whole <code>strategies</code> module — turn off default features and the crate compiles to nothing rather than to a second generator family.</p>
  </div>
  <div class="tk-card tk-card--accent">
    <span class="tk-card__kicker">Open work</span>
    <h3 class="tk-card__title">An <code>arbitrary</code> backend</h3>
    <p class="tk-card__body"><a href="https://github.com/stellar-crucible/soroban-testkit/issues/16">#16</a> adds <code>Arbitrary</code> implementations behind an <code>arbitrary</code> flag, for the fuzzers that drive inputs through that crate instead of proptest — <span class="tk-badge tk-badge--brand">Medium 150 pts</span>. Until it lands there is no such flag.</p>
  </div>
</div>
