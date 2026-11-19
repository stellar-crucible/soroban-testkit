# soroban-testkit-generators

Soroban-aware property-testing strategies for `proptest`, so generated inputs respect the ranges real contracts accept.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Features</span><span class="tk-spec__value">proptest (default)</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">strategies · differential · errors · tuples · ttl</span></div>
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
| `fuzzed_invocation_args(env, typed, max_rogue)` | `Vec<Val>` | valid arguments mixed with integers, booleans, symbols, errors, and address objects |
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
| `ed25519_signature_pair()` | `Ed25519SignaturePair` | valid ed25519 public key, message digest, and signature |
| `secp256k1_signature_pair()` | `Secp256k1SignaturePair` | valid secp256k1 public key, hash, signature, and recovery id |
| `malleated_signature(sig)` | `[u8; 64]` | corrupted signature payload guaranteed to differ |
| `contract_executable_wasm_hash()` | `[u8; 32]` | any 32-byte WASM bytecode hash |
| `contract_executable()` | `ContractExecutable` | `Wasm`, `StellarAsset`, or `ExternalRef` variants |
| `deployer_salt()` | `[u8; 32]` | 32-byte salt for deterministic deployment |
| `entry_ttl(current, min_threshold, max_ttl)` | `EntryTtl` | a live entry whose TTL is in `min_threshold..=max_ttl` |
| `expired_ttl(current)` | `EntryTtl` | an entry that expired before `current` |
| `ttl_extension_delta()` | `u32` | 1 to `MAINNET_MAX_ENTRY_TTL` (3,110,400) ledgers |
| `ttl_extension_delta_up_to(max)` | `u32` | 1 to `max` ledgers |

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

## Fuzzing invocation argument types

`fuzzed_invocation_args` takes the host environment, a vector of valid
arguments for the target function, and a maximum number of rogue values. It
returns actual host `Val`s, so object handles belong to the same environment
as the contract client. Primitive values shrink toward zero and `false`, while
the argument list shrinks toward the original typed arguments.

```rust
use soroban_sdk::{Env, IntoVal};
use soroban_testkit_generators::fuzzed_invocation_args;

let env = Env::default();
let valid = vec![7u32.into_val(&env)];
let args = fuzzed_invocation_args(&env, valid, 4);
```

Use the generated values with a contract entry point that accepts `Val` (or
through the host's dynamic invocation API) to ensure dispatch and validation
reject unexpected types before business logic runs. Generated rogue values
cover signed/unsigned integers, booleans, symbols, contract errors, and
address object handles.

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
## Entry TTLs and storage rent

Every contract data entry has a *live-until* ledger — the last ledger it can
be read on — and rent bugs sit at the edges of that interval. The `ttl` module
generates the intervals themselves, in the SDK's units: an entry's TTL is
`live_until_ledger - current_ledger`, the number `get_ttl(&key)` reports and
`extend_ttl(&key, threshold, extend_to)` takes, so a TTL of `0` means
"readable this ledger, gone the next".

```rust
use proptest::prelude::*;
use soroban_sdk::testutils::Ledger as _;
use soroban_testkit_generators::{entry_ttl, expired_ttl, ttl_extension_delta_up_to};

const RENEW_THRESHOLD: u32 = 17_280; // ~1 day
const RENEW_TO: u32 = 518_400;       // ~30 days

proptest! {
    #[test]
    fn renewal_keeps_the_balance_alive(
        entry in entry_ttl(1_000, 0, RENEW_TO),
        gap in ttl_extension_delta_up_to(RENEW_THRESHOLD),
    ) {
        // Put the entry on the ledger with exactly entry.ttl() remaining,
        // then call the contract's renewal function.
        client.renew();
        let ttl = env.as_contract(&id, || env.storage().persistent().get_ttl(&KEY));
        prop_assert!(ttl >= RENEW_THRESHOLD);

        env.ledger().set_sequence_number(env.ledger().sequence() + gap);
        prop_assert!(client.has_balance());
    }

    #[test]
    fn an_expired_entry_is_never_read(entry in expired_ttl(50_000)) {
        prop_assert!(!entry.is_live());
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

`EntryTtl` carries `current_ledger` and `live_until_ledger`, and answers the
questions a rent test asks of them: `is_live()`, `ttl()` (`None` once expired),
`ledgers_since_expiry()`, `expires_at()` (the first unreadable ledger) and
`advanced_by(n)` for the same entry observed `n` ledgers later.

`max_ttl` is the network ceiling — `MAINNET_MAX_ENTRY_TTL` on Mainnet, or the
value a test passed to `env.ledger().set_max_entry_ttl(..)` — and an interval
that would run past `u32::MAX` is clamped. `entry_ttl` refuses a
`min_threshold` above `max_ttl`, and `expired_ttl` refuses ledger `0`, because
neither has a value to generate.

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking goes to the expiration boundary</span>
  <p><code>entry_ttl</code> shrinks the TTL towards <code>min_threshold</code>, <code>expired_ttl</code> shrinks towards the entry that expired exactly one ledger ago, and <code>ttl_extension_delta</code> shrinks towards a single ledger. A renewal bug therefore minimises to the entry closest to dying, which is where off-by-one liveness checks fail. The module's own tests check these intervals against the host: what <code>entry_ttl</code> generates is what <code>get_ttl</code> reports, and a temporary entry is readable on <code>live_until_ledger</code> and gone on <code>expires_at()</code>.</p>
</div>

## Event Fuzzing

`EventFuzzHarness` produces valid SDK v28 `ContractEvent` values and mutates
them to verify that event parsers reject malformed payloads cleanly.

```rust
use soroban_testkit_generators::event_fuzz::{
    fuzzed_event, valid_transfer_event, EventFuzzHarness, MutationOperator,
};

// Start from a well-formed event and apply a single mutation.
let harness = EventFuzzHarness::new(valid_transfer_event());
let malformed = harness.mutate(MutationOperator::DropField, 42);

// Or drive a property test across every mutation operator.
proptest! {
    #[test]
    fn parser_never_panics(event in fuzzed_event(valid_transfer_event())) {
        // Downstream decoding must return an error, never panic.
        let _ = TransferEvent::try_from_event(&event);
    }
}
```

Supported mutation operators:

| Operator | Effect |
|----------|--------|
| `DropField` | Removes a topic, or clears the data when no topics remain |
| `TypeShift` | Replaces a value with a different `ScVal` variant |
| `ByteTruncate` | Halves the length of bytes, strings, symbols, or vectors |
| `SignFlip` | Flips the sign of a number or toggles a boolean |

`EventFuzzHarness::shrink` deterministically reduces a malformed payload after
a failure, mirroring proptest's shrinking so failures are reported with the
smallest event that still reproduces the bug. `shrink_candidates` returns each
intermediate step for debugging.

## Contract executables and deployer salts

Factory contracts and deployment utilities dynamically deploy child contracts using WASM hashes or contract executable descriptors. `contract_executable_wasm_hash()`, `contract_executable()`, and `deployer_salt()` test factory patterns and verify deterministic contract address calculation:

```rust
use proptest::prelude::*;
use soroban_sdk::{Address, BytesN, Env};
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn deterministic_factory_address(
        salt in strategies::deployer_salt(),
        wasm_hash in strategies::contract_executable_wasm_hash(),
    ) {
        let env = Env::default();
        let deployer = Address::generate(&env);
        let salt_n = BytesN::from_array(&env, &salt);

        // Precompute deterministic deployment address
        let predicted = env
            .deployer()
            .with_address(deployer.clone(), salt_n.clone())
            .deployed_address();

        // Repeated address calculations with identical inputs match
        let predicted_repeat = env
            .deployer()
            .with_address(deployer, salt_n)
            .deployed_address();

        assert_eq!(predicted, predicted_repeat);
    }
}
```

`contract_executable()` generates variants across `ContractExecutable::Wasm`, `ContractExecutable::StellarAsset`, and `ContractExecutable::ExternalRef`, allowing comprehensive fuzzing of contract installation and factory routines.

## Cryptographic signatures and malleation

Contracts verifying external cryptographic signatures with `env.crypto().ed25519_verify` or `secp256k1_recover` require testing both valid signatures and corrupted variants.

`ed25519_signature_pair()` generates cryptographically valid Ed25519 keypairs, message digests, and signatures. `secp256k1_signature_pair()` produces valid Secp256k1 SEC-1 uncompressed 65-byte public keys, message hashes, 64-byte compact ECDSA signatures, and recovery IDs:

```rust
use proptest::prelude::*;
use soroban_sdk::{Bytes, BytesN, Env};
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn verify_ed25519_signatures(pair in strategies::ed25519_signature_pair()) {
        let env = Env::default();
        let pk = BytesN::from_array(&env, pair.public_key());
        let msg = Bytes::from_slice(&env, pair.message_digest());
        let sig = BytesN::from_array(&env, pair.signature());

        // Verifies cleanly without panic
        env.crypto().ed25519_verify(&pk, &msg, &sig);
    }

    #[test]
    fn recover_secp256k1_public_keys(pair in strategies::secp256k1_signature_pair()) {
        let env = Env::default();
        let hash_bytes = Bytes::from_slice(&env, pair.message_hash());
        let hash = env.crypto().sha256(&hash_bytes);
        let sig = BytesN::from_array(&env, pair.signature());

        let recovered = env.crypto().secp256k1_recover(&hash, &sig, pair.recovery_id());
        assert_eq!(recovered.to_array(), *pair.public_key());
    }
}
```

`malleated_signature(valid_sig)` introduces single-bit or byte perturbations into valid signatures to verify that corrupted or tampered signatures fail verification cleanly.

## Differential comparison

A strategy generates inputs; `DifferentialHarness` generates them *and* runs a second implementation of the same rule, so the assertion is agreement rather than a property you had to think of. It runs its own `proptest` loop, so it belongs in an ordinary `#[test]` rather than inside a `proptest!` block.

```rust
use soroban_testkit_generators::differential::DifferentialHarness;
use soroban_testkit_generators::strategies::token_amount;

#[test]
fn fee_router_matches_the_specified_fee() {
    let env = Env::default();
    let id = env.register(FeeRouter, ());
    let client = FeeRouterClient::new(&env, &id);

    DifferentialHarness::new()
        .cases(64)
        .tolerance(1)
        .compare(token_amount(), |amount| client.calculate_fee(&amount), reference_fee);
}
```

`compare` asserts and panics with the shrunk input; `diff` runs the same loop and returns a `DifferentialReport` you inspect instead. Both closures take the generated value by value and return anything convertible into a trace: a primitive, a `Vec<u8>`, an `Option`, or an `ExecutionTrace` that also names the storage keys the call left behind. `tolerance` is in absolute units and applies only where both sides are numeric, so a `Divergence` carries the `DivergenceKind` (`Return` or `State { key }`), both values, and their delta where one exists.

<div class="tk-callout">
  <span class="tk-callout__title">Model it, don't copy it</span>
  <p>The reference closure has to come from the specification. A second copy of the contract's body agrees with it case for case, including on the case that is wrong, and the run proves nothing. <a href="../guides/property-testing.html">Property Testing</a> covers what shrinking does and does not promise about the payload it prints.</p>
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
