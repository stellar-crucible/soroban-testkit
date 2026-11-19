# soroban-testkit-generators

Soroban-aware property-testing strategies for `proptest`, so generated inputs respect the ranges real contracts accept.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Features</span><span class="tk-spec__value">proptest (default)</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Module</span><span class="tk-spec__value">strategies</span></div>
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
| `fuzzed_scval_map(max_entries)` | `ScVal` | a map of at most `max_entries` entries, sorted or deliberately unsorted |
| `fuzzed_scval_vec(max_entries)` | `ScVal` | a vector of at most `max_entries` elements, including null objects |
| `malformed_object_payload()` | `Vec<u8>` | a real map or vector's XDR, truncated at a generated offset |

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

## XDR objects and malformed payloads

A contract that decodes an object parameter is a parser, and parsers fail on
shapes a hand-written test never tries: a map whose keys are not in ledger
order, a map with duplicate keys, a vector longer than the host accepts, a null
object where a map was expected, and a payload that simply stops in the middle
of an entry. The three object strategies generate exactly those.

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies::{
    fuzzed_scval_map, fuzzed_scval_vec, malformed_object_payload,
};

proptest! {
    #[test]
    fn a_parser_never_panics_on_a_map(map in fuzzed_scval_map(16)) {
        // map: ScVal::Map(Some(..)), sorted or unsorted, at most 16 entries
    }

    #[test]
    fn an_oversized_vector_is_refused_not_allocated(vector in fuzzed_scval_vec(4_096)) {
        // vector: ScVal::Vec(Some(..)); pass a multiple of the host's element
        // limit to exercise the refusal path
    }

    #[test]
    fn a_truncated_payload_is_an_error_not_a_crash(payload in malformed_object_payload()) {
        // payload: a real object's XDR with its tail cut off, 0..len bytes
    }
}
```

All three are built out of proptest collections rather than raw bytes, so
shrinking is meaningful: a failure reports the smallest map, the shortest
vector, or the shortest prefix that still fails, instead of the case the
runner happened to draw. `DEFAULT_FUZZ_ENTRIES` is the budget the two object
strategies are usually given when a suite has no opinion.

| Strategy | What is generated | What it catches |
|----------|-------------------|-----------------|
| `fuzzed_scval_map(n)` | `ScVal::Map(Some(..))` with 0..=n entries; half sorted and key-unique, half in generation order | a parser that assumes ledger ordering or unique keys |
| `fuzzed_scval_vec(n)` | `ScVal::Vec(Some(..))` with 0..=n elements, including `Vec(None)` and `Map(None)` values | a parser that unwraps `Option<ScVal>` arity or allocates before checking the length |
| `malformed_object_payload()` | `Vec<u8>`: a real object's XDR cut at a generated offset, from an empty payload to one byte short of complete | a decoder that panics on a truncated tag or a length that promises more bytes than follow |

<div class="tk-callout">
  <span class="tk-callout__title">An unsorted map is the interesting half</span>
  <p>The host validates a <code>ScMap</code> before a contract sees one, so a contract that reads ledger state is safe by construction. The strategy still generates unsorted maps because a parser can receive entries from a client, an oracle or a nested record that never went through that check — and because the test that proves the assumption fails is the one that documents the boundary.</p>
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
