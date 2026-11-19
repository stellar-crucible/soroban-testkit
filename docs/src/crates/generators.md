# soroban-testkit-generators

Soroban-aware property-testing strategies for `proptest`, so generated inputs respect the ranges real contracts accept.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Features</span><span class="tk-spec__value">proptest (default)</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">strategies · errors · tuples</span></div>
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
| `soroban_tuple_2(a, b)` | `(A, B)` | one component from each strategy |
| `soroban_tuple_3(a, b, c)` | `(A, B, C)` | one component from each strategy |
| `byte_array_32()` | `[u8; 32]` | any 32 bytes — hashes, keys, seeds |
| `byte_array_64()` | `[u8; 64]` | any 64 bytes — ed25519 signatures |
| `byte_array::<N>()` | `[u8; N]` | any `N` bytes |

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

## Tuples and fixed-size arrays

Contract functions often take a composite argument — an `(Address, i128)`
allowance grant, a `BytesN<32>` hash, a `BytesN<64>` signature. The `tuples`
module names those shapes so a test does not rebuild them by hand, and every
strategy is re-exported from the crate root:

```rust
use proptest::prelude::*;
use soroban_sdk::{BytesN, Env};
use soroban_testkit_generators::strategies::{account_address, token_amount};
use soroban_testkit_generators::{byte_array_32, byte_array_64, soroban_tuple_2};

proptest! {
    #[test]
    fn test_allowance_pair(
        (owner, spender) in soroban_tuple_2(account_address(), account_address()),
        amount in token_amount(),
    ) {
        let env = Env::default();
        let (owner, spender) = (owner.to_address(&env), spender.to_address(&env));
        client.approve(&owner, &spender, &amount);
    }

    #[test]
    fn verify_rejects_a_forged_signature(
        digest in byte_array_32(),
        signature in byte_array_64(),
    ) {
        let env = Env::default();
        let digest = BytesN::from_array(&env, &digest);
        let signature = BytesN::from_array(&env, &signature);
        // ...
    }
}
```

The values are plain Rust tuples and arrays, so once each component is a host
type the whole value converts with `soroban_sdk::IntoVal` — `(Address, i128)`,
`(Address, Address, i128)` and `BytesN<N>` all cross a contract boundary
unchanged, which is what the module's own tests check against a live contract.
`soroban_tuple_3(a, b, c)` is the three-component form, and `byte_array::<N>()`
covers any other length.

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking is per component</span>
  <p>A failing tuple shrinks one component at a time while the others stay put, and a failing byte array shrinks every byte towards <code>0x00</code> independently. A signature check that only trips on the last byte therefore minimises to sixty-three zeros and the one byte that matters, rather than to an arbitrary-looking blob.</p>
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
