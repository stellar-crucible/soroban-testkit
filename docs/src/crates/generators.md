# soroban-testkit-generators

Soroban-aware property-testing strategies for `proptest`, so generated inputs respect the ranges real contracts accept.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Package</span><span class="tk-spec__value">soroban-testkit-generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Features</span><span class="tk-spec__value">proptest (default) · arbitrary</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Modules</span><span class="tk-spec__value">strategies · contract_struct · xdr · arbitrary · errors · tuples · ttl · storage_tier · cost_model · financial</span></div>
</div>

## Available strategies

| Strategy | Type | Range |
|----------|------|-------|
| `token_amount()` | `i128` | 0 to 1e15 |
| `stroop_amount()` | `i128` | 0 to `MAX_TOKEN_SUPPLY` (1e9 units), weighted towards decimal boundaries |
| `extreme_stroop_amount()` | `i128` | stroops plus `i64::MAX`, `u64::MAX` and `i128::MAX` |
| `stroop_boundaries()` | `Vec<i128>` | the amounts a fee rounds wrongly around: 0, 1 stroop, a unit ± 1, ten units ± 1, the cap |
| `basis_points()` | `u32` | 0 to 10,000, weighted towards 0, 1, 30, 50, 100, 9,999 and 10,000 |
| `exchange_rate_ratio()` | `(i128, i128)` | numerator and denominator in 1 to 1e6, so the supply cap times either leg fits in `i128` |
| `priced_stroop_amount()` | `(i128, i128, i128)` | `(amount, numerator, denominator)` — an amount with the rate that prices it |
| `split_fee(amount, bps)` | `(i128, i128)` | reference `(net, fee)` split, rounding the fee up |
| `ledger_sequence()` | `u32` | 1 to 10,000,000 |
| `timestamp()` | `u64` | Sep 2020 to May 2033 |
| `address()` | `AddressBytes` | an account or contract address |
| `account_address()` | `AddressBytes` | an ed25519 account (`G...`) |
| `contract_address()` | `AddressBytes` | a contract id (`C...`) |
| `address_of_type(kind)` | `AddressBytes` | the format you ask for |
| `sc_address()` | `ScAddress` | a `ScAddress::Account`, `ScAddress::Contract`, or `ScAddress::MuxedAccount` |
| `sc_address_account()` | `ScAddress` | a `ScAddress::Account` (`G...`) address |
| `sc_address_contract()` | `ScAddress` | a `ScAddress::Contract` (`C...`) address |
| `sc_address_muxed()` | `ScAddress` | a `ScAddress::MuxedAccount` (`M...`) address |
| `sc_address_collection()` | `Vec<ScAddress>` | a mixed vector holding both account and contract addresses |
| `fuzzed_invocation_args(env, typed, max_rogue)` | `Vec<Val>` | valid arguments mixed with integers, booleans, symbols, errors, and address objects |
| `contract_error_code()` | `u32` | any application-defined contract error code, shrinking towards zero |
| `host_error()` | `Error` | an SDK error from every standard category, including arbitrary contract codes |
| `error_with_type(kind)` | `Error` | an SDK error restricted to the requested `ScErrorType` |
| `host_error_code_coverage()` | `HostError` | all 10 categories × all 10 codes |
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
| `storage_tier()` | `StorageTier` | `Instance`, `Persistent`, or `Temporary` (shrinks to `Temporary`) |
| `storage_tier_with_ttl(current, min, max)` | `StorageTierWithTtl` | a tier paired with durability and live/expired TTL properties |
| `mixed_tier_allocations(keys, size)` | `MixedTierAllocation<K>` | mixed key allocations across instance, persistent, and temporary tiers |
| `mixed_tier_allocations_with_ttl(keys, current, min, max, size)` | `MixedTierTtlAllocation<K>` | mixed key allocations paired with TTLs per tier |
| `entry_ttl(current, min_threshold, max_ttl)` | `EntryTtl` | a live entry whose TTL is in `min_threshold..=max_ttl` |
| `expired_ttl(current)` | `EntryTtl` | an entry that expired before `current` |
| `ttl_extension_delta()` | `u32` | 1 to `MAINNET_MAX_ENTRY_TTL` (3,110,400) ledgers |
| `ttl_extension_delta_up_to(max)` | `u32` | 1 to `max` ledgers |
| `soroban_duration(env, min_secs, max_secs)` | `Duration` | a host duration in `min_secs..=max_secs` seconds |
| `soroban_timepoint(env, min_timestamp, max_timestamp)` | `Timepoint` | a host timepoint in `min_timestamp..=max_timestamp` unix seconds |
| `ledger_sequence_interval()` | `(u32, u32)` | `(start, end)` with `start <= end`, both in `1..=10,000,000` |
| `contract_cost_model()` | `ContractCostModel` | valid host cost schedule covering all `ContractCostType` variants, shrinking to baseline |
| `contract_cost_model_with_multipliers(cpu, mem)` | `ContractCostModel` | cost schedule with custom CPU and Memory fee multipliers (percentage) |
| `contract_cost_model_with_config(config)` | `ContractCostModel` | cost schedule with per-dimension and per-cost-type multiplier overrides |
| `soroban_symbol(env)` | `Symbol` | valid alphanumeric and underscore symbol (1 to 32 chars) |
| `sorobans_symbol_strategy(env, min, max)` | `Symbol` | valid symbol constrained to custom length range `min..=max` |
| `soroban_identifier_symbol(env)` | `Symbol` | standard naming convention identifier starting with letter |
| `invalid_symbol_strategy()` | `String` | strings with rejected/invalid characters (spaces, special symbols, etc.) |
| `xdr_symbol(env, max_len)` | `Symbol` | `[a-zA-Z0-9_]`, at most `max_len` (≤ 32) characters |
| `xdr_string(env, max_len)` | `String` | printable ASCII, at most `max_len` bytes |
| `xdr_bytes(env, max_len)` | `Bytes` | at most `max_len` bytes |
| `xdr_vec(env, element, max_len)` | `Vec<T>` | at most `max_len` elements drawn from any XDR strategy |
| `xdr_map(env, key, value, max_len)` | `Map<K, V>` | at most `max_len` entries; duplicate keys collapse |
| `xdr_value(bounds)` | `XdrValue` | an env-free value tree inside an `XdrBounds` |
| `xdr_val(env, bounds)` | `Val` | a host value built from an `xdr_value` tree |

## Money: stroops, basis points and rates

`financial` spends its cases where integer money goes wrong instead of spreading
them evenly over `i128`. Two thirds of the amounts come from
`stroop_boundaries()` — zero, one stroop, one stroop either side of a whole unit,
a unit either side of ten, and the supply cap — and the rest is spread over
`0..=MAX_TOKEN_SUPPLY` (1e9 units at seven decimals). Rate ratios stay in
`1..=1e6` on both legs, so `MAX_TOKEN_SUPPLY * numerator` is always representable:
a failed case means the contract overflowed, not the strategy.

```rust
use proptest::prelude::*;
use soroban_testkit_generators::financial::{basis_points, stroop_amount};
use soroban_testkit_generators::{assert_balances_never_negative, assert_fee_rounding_favors_protocol};

proptest! {
    #[test]
    fn a_swap_keeps_every_stroop(amount in stroop_amount(), fee_bps in basis_points()) {
        let (net, fee) = split(amount, fee_bps);
        assert_fee_rounding_favors_protocol!(gross = amount, fee_bps = fee_bps, net = net, fee = fee);
        assert_balances_never_negative!(net = net, fee = fee);
    }
}
```

`assert_fee_rounding_favors_protocol!` holds a split to the three things a fee
bug breaks: the legs add back up to the gross, neither is negative, and the fee
is no more than one stroop away from `gross * bps / 10_000` in either direction —
the loosest two honest divisions can be. `assert_balances_never_negative!` takes
as many `name = value` pairs as you have legs and names the one that went below
zero.

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

## Strkeys (account, contract, corrupted)

Soroban strkeys are 56-character base32 strings carrying a version byte and a
CRC-16 checksum. The generators module provides strategies that produce valid
account (`G...`) and contract (`C...`) strkeys, plus corrupted variants that
flip a checksum bit:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies::{
    account_strkey, contract_strkey, corrupted_strkey, Strkey,
};

proptest! {
    #[test]
    fn round_trips(s in account_strkey()) {
        let decoded = Strkey::decode(&s).unwrap();
        assert_eq!(decoded.encoded(), s);
    }

    #[test]
    fn corrupted_is_rejected(s in corrupted_strkey()) {
        assert!(Strkey::decode(&s).is_err());
    }
}
```

`Strkey` exposes `new`, `zero`, `encoded`, `decode`, `to_address`, and
`StrkeyKind`/`StrkeyError` for direct parsing tests. Valid inputs shrink toward
the canonical all-zero strkey; corrupted inputs shrink toward a single-bit
checksum flip.

## ScAddress (account, contract, and muxed addresses)

`address()` and friends produce host `AddressBytes`, which is convenient for
contract calls but hides the XDR shape. When a test compares against ledger
values, decodes strkeys, or checks low-level conversion code, generate the raw
`ScAddress` instead:

* `sc_address_account()` — `ScAddress::Account`, an Ed25519 account id (`G...`).
* `sc_address_contract()` — `ScAddress::Contract`, a 32-byte contract id hash (`C...`).
* `sc_address_muxed()` — `ScAddress::MuxedAccount`, an Ed25519 key plus a `u64`
  id, the strkey `M...` form.
* `sc_address()` — one of the three variants.
* `sc_address_collection()` — a `Vec<ScAddress>` that always holds at least one
  account and one contract address, plus up to six more entries of either
  variant.

```rust
use proptest::prelude::*;
use soroban_sdk::xdr::ScAddress;
use soroban_testkit_generators::strategies::{sc_address, sc_address_collection};

proptest! {
    #[test]
    fn every_variant_round_trips_through_its_strkey(address in sc_address()) {
        let encoded = address.to_string();
        let decoded: ScAddress = encoded.parse().expect("generated strkey must decode");
        assert_eq!(decoded, address);
    }

    #[test]
    fn mixed_collections_cover_both_xdr_formats(addresses in sc_address_collection()) {
        assert!(addresses.iter().any(|a| matches!(a, ScAddress::Account(_))));
        assert!(addresses.iter().any(|a| matches!(a, ScAddress::Contract(_))));
    }
}
```

Account and contract values convert both ways with the host type, which is how
you move from generated XDR to a call argument:

```rust
use soroban_sdk::{Address, Env, TryFromVal};
use soroban_sdk::xdr::ScAddress;
use soroban_testkit_generators::sc_address_account;

let env = Env::default();
// ...inside a property, with `sc: ScAddress` from `sc_address_account()`...
let address = Address::try_from_val(&env, &sc).unwrap();
assert_eq!(ScAddress::from(&address), sc);
```

Muxed accounts have no host `Address` counterpart, so encode them to their
strkey when a test needs to pass them through a textual interface.

All of these strategies shrink toward zeroed payloads: the account key and the
contract hash become `[0u8; 32]`, and the muxed id becomes `0`. There is no
uniqueness nonce here (unlike `address()`), so the shrink target really is the
canonical all-zero value.

## Generating strategies for nested contract structs

The `prop_contract_struct!` macro synthesises a proptest strategy plus a
builder function for a user `#[contracttype]` struct. You provide one strategy
per field; the macro maps the tuple of field values into the struct, so each
field keeps its own shrink tree and shrinking recurses through nested members.

```rust
use proptest::prelude::*;
use soroban_sdk::{contracttype, Env};
use soroban_testkit_generators::{prop_contract_struct, strategies};

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    pub amount: i128,
    pub enabled: bool,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Order {
    pub limit: Limit,
    pub backup: Option<i128>,
    pub tags: soroban_sdk::Vec<u32>,
}

prop_contract_struct! {
    pub fn limit_strategy() -> Limit {
        amount: strategies::token_amount(),
        enabled: any::<bool>(),
    }
}

prop_contract_struct! {
    pub fn order_strategy(env: Env) -> Order {
        limit: limit_strategy(),
        backup: prop::option::of(strategies::token_amount()),
        tags: strategies::soroban_vec(0u32..=100, 4, env),
    }
}
```

The generated function accepts whatever parameters the invocation declares and
forwards them into the field strategy expressions, which is how
`strategies::soroban_vec` receives its `Env`. Attributes, doc comments, and
visibility are copied onto the generated function.

Child structs are ordinary strategies, so they compose with `prop::option::of`,
`prop::collection::vec`, or `prop::collection::hash_map`. Contract enums work
the same way: build a `prop_oneof!` strategy for the enum and use it as the
field strategy.

The macro builds a proptest tuple, which supports up to 12 fields per struct.
Group wider structs into a child `#[contracttype]` struct to stay inside that
limit.

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

Use `host_error()` when the code under test consumes the SDK error directly;
`error_with_type(kind)` keeps generated errors within one category, including
application-defined contract codes when `kind` is `ScErrorType::Contract`.
`contract_error_code()` produces the entire valid `u32` range and shrinks
failing values towards zero. Generated SDK errors convert to the corresponding
XDR [`ScVal::Error`](https://docs.rs/soroban-sdk/latest/soroban_sdk/xdr/enum.ScVal.html)
without losing their category or code:

```rust
use proptest::prelude::*;
use soroban_sdk::xdr::{ScErrorType, ScVal};
use soroban_testkit_core::error::DecodedError;
use soroban_testkit_generators::{error_with_type, host_error};

proptest! {
    #[test]
    fn error_handler_never_panics(error in host_error()) {
        let decoded = DecodedError::from_error(&error);
        assert!(!decoded.category.is_empty());

        let xdr_error = ScVal::try_from(error).expect("generated errors are valid XDR");
        assert!(matches!(xdr_error, ScVal::Error(_)));
    }

    #[test]
    fn storage_error_handler_never_panics(
        error in error_with_type(ScErrorType::Storage),
    ) {
        // The strategy only emits Storage errors.
        assert!(error.is_type(ScErrorType::Storage));
    }
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking goes to the smallest discriminant</span>
  <p><code>host_error_code_coverage()</code> uses two bounded integers — one position in the category table, one in the code table — rather than a <code>prop_oneof!</code> over the pairs. proptest shrinks integers towards zero, and both tables are in XDR discriminant order, so a failing case simplifies to <code>Contract</code>/<code>ArithDomain</code> (<code>contract/0</code>). <code>host_error()</code> follows that category order and contract codes shrink towards zero too.</p>
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
## Host-allocated collection generators

Soroban contracts work with host-allocated collection types (`Map`, `Vec`, `Bytes`, `String`) that live in the contract's environment. The `strategies` module provides generators that construct these types directly in the test's `Env`, with configurable size bounds and proper shrinking.

### Map generator

```rust
use proptest::prelude::*;
use soroban_sdk::{Env, Map};
use soroban_testkit_generators::{soroban_map, token_amount};

proptest! {
    #[test]
    fn test_batch_process(
        ref map in soroban_map(token_amount(), token_amount(), 20, Env::default())
    ) {
        // map is a soroban_sdk::Map<i128, i128> with at most 20 entries
        prop_assert!(map.len() <= 20);
    }
}
```

**Signature:**
```rust
soroban_map<K, V, KS, VS>(key_strategy: KS, value_strategy: VS, max_entries: u32, env: Env)
```

- `key_strategy` / `value_strategy`: proptest strategies for keys and values
- `max_entries`: maximum number of entries (shrinking reduces entry count)
- `env`: the Soroban environment to allocate the map in

### Vec generator

```rust
use proptest::prelude::*;
use soroban_sdk::{Env, Vec};
use soroban_testkit_generators::{soroban_vec, token_amount};

proptest! {
    #[test]
    fn test_batch_process(
        ref vec in soroban_vec(token_amount(), 50, Env::default())
    ) {
        // vec is a soroban_sdk::Vec<i128> with at most 50 elements
        prop_assert!(vec.len() <= 50);
    }
}
```

**Signature:**
```rust
soroban_vec<T, S>(elem_strategy: S, max_entries: u32, env: Env)
```

### Bytes generator

```rust
use proptest::prelude::*;
use soroban_sdk::{Bytes, Env};
use soroban_testkit_generators::soroban_bytes;

proptest! {
    #[test]
    fn test_bytes_processing(
        ref bytes in soroban_bytes(256, Env::default())
    ) {
        // bytes is a soroban_sdk::Bytes with at most 256 bytes
        prop_assert!(bytes.len() <= 256);
    }
}
```

**Signature:**
```rust
soroban_bytes(max_len: u32, env: Env)
```

### String generator

```rust
use proptest::prelude::*;
use soroban_sdk::{String, Env};
use soroban_testkit_generators::{soroban_string, soroban_ascii_string};

proptest! {
    #[test]
    fn test_string_processing(
        ref s in soroban_string(128, Env::default())
    ) {
        // s is a soroban_sdk::String with at most 128 bytes
        prop_assert!(s.len() <= 128);
    }

    #[test]
    fn test_ascii_string(
        ref s in soroban_ascii_string(64, Env::default())
    ) {
        // s is a soroban_sdk::String with only printable ASCII, up to 64 bytes
        prop_assert!(s.len() <= 64);
    }
}
```

**Signatures:**
```rust
soroban_string(max_len: u32, env: Env) -> Strategy<Value = soroban_sdk::String>
soroban_ascii_string(max_len: u32, env: Env) -> Strategy<Value = soroban_sdk::String>
```

- `soroban_string`: generates printable ASCII strings (valid UTF-8)
- `soroban_ascii_string`: generates only printable ASCII characters (subset of `soroban_string`)

All collection generators:
- Respect the configured size bounds
- Shrink towards empty collections (reducing entries/elements/bytes)
- Construct the collection directly in the provided `Env`
- Work with any key/value/element types that implement `IntoVal<Env, Val>` and `TryFromVal<Env, Val>`

## Stateful property-based testing with state machines

The `state_machine` module provides a Soroban-aware state machine harness built on `proptest::state_machine`. It enables testing contracts with sequences of transactions, verifying invariants after each step, and automatically shrinking failing sequences to the minimal reproducible call sequence.

### Defining a state machine

```rust
use soroban_testkit_generators::state_machine::{SorobanStateMachine, SorobanStateMachineRunner, prop_state_machine_test};
use soroban_sdk::{Env, Address, testutils::Address as _};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TokenModel {
    balances: std::collections::HashMap<Address, i128>,
    total_supply: i128,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TokenAction {
    Mint { to: Address, amount: i128 },
    Transfer { from: Address, to: Address, amount: i128 },
    Burn { from: Address, amount: i128 },
}

struct TokenStateMachine {
    env: Env,
}

impl SorobanStateMachine for TokenStateMachine {
    type Action = TokenAction;
    type Model = TokenModel;
    type Context = TokenTestContext;

    fn generate_action(&self, model: &Self::Model) -> BoxedStrategy<Self::Action> {
        // Generate actions based on current model state
    }

    fn check_precondition(&self, model: &Self::Model, action: &Self::Action) -> bool {
        // Validate action against model state
    }

    fn execute(&self, context: &mut Self::Context, action: &Self::Action) -> Result<(), String> {
        // Execute action against real contract
    }

    fn apply_to_model(&self, model: &mut Self::Model, action: &Self::Action) {
        // Update expected model state
    }

    fn check_invariant(&self, model: &Self::Model, context: &Self::Context) -> TestCaseResult {
        // Verify invariant (e.g., total supply conservation)
    }

    fn initial_model(&self) -> Self::Model {
        TokenModel::default()
    }

    fn create_context(&self) -> Self::Context {
        TokenTestContext { ... }
    }
}
```

### Running the state machine

```rust
let machine = TokenStateMachine::new();
let runner = SorobanStateMachineRunner::new(machine).cases(100);
runner.run().unwrap();
```

Or use the simpler `prop_state_machine_test` function:

```rust
prop_state_machine_test(machine, 20, 100).unwrap();
```

### Key features

- **Automated invariant verification** after each transition
- **Precondition checking** to filter invalid actions
- **Automated shrinking** of failing action sequences to the minimal failing sequence
- **Sequence length ≥ 50** actions supported
- **Integration example**: Token balance conservation invariant verification

## State machine for property testing

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

## Durations, timepoints and ledger intervals

Vesting schedules, timelocks and renewal windows are all expressed in the
SDK's two time types. Both are 64-bit unsigned second counts, but they are
environment-backed wrappers rather than `u64`: build one with
`Duration::from_seconds(&env, ..)` or `Timepoint::from_unix(&env, ..)` and read
it back with `to_seconds()` or `to_unix()`. The strategies therefore take an
`Env`, the same way `fuzzed_invocation_args` does, so the generated value
belongs to the environment under test.

```rust
use proptest::prelude::*;
use soroban_sdk::Env;
use soroban_testkit_generators::{ledger_sequence_interval, soroban_duration, soroban_timepoint};

proptest! {
    #[test]
    fn a_timelock_unlocks_after_its_start(
        (start, duration) in {
            let env = Env::default();
            (
                soroban_timepoint(&env, 1_600_000_000, 2_000_000_000),
                soroban_duration(&env, 0, 100_000_000),
            )
        },
        (first, last) in ledger_sequence_interval(),
    ) {
        let unlock = start.to_unix() + duration.to_seconds(); // bounded ranges
        prop_assert!(unlock >= start.to_unix());
        prop_assert!(first <= last);
    }
}
```

`ledger_sequence_interval()` returns the interval itself rather than a
`Duration` or `Timepoint`: both ends are ordinary `u32` ledger numbers in the
same range `ledger_sequence()` draws from, ordered so `start <= end` always
holds.

<div class="tk-callout tk-callout--info">
  <span class="tk-callout__title">Ranges are validated</span>
  <p><code>soroban_duration</code> and <code>soroban_timepoint</code> panic when the minimum is above the maximum, matching the crate's existing range strategies. The whole <code>u64</code> range is accepted, including <code>u64::MAX</code>; the range strategy itself cannot overflow because it never adds to the generated value.</p>
</div>

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking goes to the shortest span</span>
  <p>A failing duration minimises to <code>min_secs</code>, a failing timepoint to <code>min_timestamp</code>, and a failing interval to <code>(1, 1)</code> — or to <code>(1, 2)</code> when the bug needs a non-empty span. A vesting calculation that is wrong at the end of its schedule is therefore reported at the earliest boundary that reproduces it rather than at a random year.</p>
</div>

## Realistic ledger windows

A time-locked contract sees both `env.ledger().sequence()` and
`env.ledger().timestamp()`, and the two move together: Stellar closes a ledger
about every five seconds. `realistic_ledger_window()` generates a sequence span
and the timestamps at its two ends, correlated at that cadence with a bounded
clock skew:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::{realistic_ledger_window, LedgerWindow};

proptest! {
    #[test]
    fn a_timelock_unlocks_inside_its_window(
        window in realistic_ledger_window(),
        lock_secs in 0u64..=600,
    ) {
        let unlock_at = window.start_timestamp + lock_secs;
        let unlock_ledger = window.sequence_at(unlock_at);
        prop_assert!(window.timestamp_at(unlock_ledger) <= unlock_at);
    }
}
```

`LedgerWindow` carries `start_sequence`, `end_sequence`, `start_timestamp`,
`end_timestamp` and the `skew_secs` beyond the nominal cadence, with helpers:

| Method | Meaning |
|--------|---------|
| `ledger_count()` | Ledgers spanned; zero for an immediate window |
| `elapsed_secs()` | Network time spanned, skew included |
| `is_immediate()` | The window opens and closes on the same ledger |
| `timestamp_at(seq)` | Clock at a ledger, interpolated across the window |
| `sequence_at(ts)` | The ledger at or immediately before a timestamp |

Both ends are inclusive, so `timestamp_at(start_sequence)` is
`start_timestamp` and `timestamp_at(end_sequence)` is `end_timestamp`. Set them
on a test environment with
`env.ledger().set_sequence_number(window.end_sequence)` and
`env.ledger().set_timestamp(window.end_timestamp)` to run a contract at the far
end of the window.

`skew_secs` lies in `0..=ledger_count()`, so the generated window averages
between five and six seconds per ledger; the end timestamp is never behind the
start. Start sequences come from the same range as `ledger_sequence()`, and the
span reaches `MAX_LEDGER_WINDOW_LEDGERS` (about six days of network time).

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking goes to immediate execution</span>
  <p>A failing window minimises to a zero-ledger, zero-second span with no skew, at the lowest start sequence and <code>MIN_LEDGER_TIMESTAMP</code>. A boundary check that is wrong only for a long lock therefore reports the smallest window that still reproduces it, rather than a random week of ledgers.</p>
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

## Storage tiers and allocations

Property-testing generalized storage adapters, multi-tier caches, or state partitioning layers requires generating random storage tier allocations (`Instance`, `Persistent`, `Temporary`) to test that functions handle all three tiers correctly.

`storage_tier()` produces all standard `StorageTier` enum variants compatible with `soroban-testkit-core`. It shrinks towards `StorageTier::Temporary` so that failing test cases simplify to the most ephemeral storage tier:

```rust
use proptest::prelude::*;
use soroban_testkit_core::storage::StorageTier;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn cache_adapter_handles_all_tiers(tier in strategies::storage_tier()) {
        match tier {
            StorageTier::Temporary => { /* ephemeral storage handling */ }
            StorageTier::Persistent => { /* restorable long-lived storage */ }
            StorageTier::Instance => { /* contract instance storage */ }
        }
    }
}
```

### Storage tiers with TTL properties

`storage_tier_with_ttl(current_ledger, min_threshold, max_ttl)` pairs each generated tier with its protocol durability (`ContractDataDurability`) and entry TTL properties (`EntryTtl`):

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn storage_renewal_respects_durability(
        alloc in strategies::storage_tier_with_ttl(1_000, 100, 10_000),
    ) {
        assert!(alloc.is_live());
        if alloc.is_temporary() {
            assert!(!alloc.is_restorable());
        } else if alloc.is_persistent() {
            assert!(alloc.is_restorable());
        }
    }
}
```

### Mixed-tier key allocations

`mixed_tier_allocations(key_strategy, count_range)` generates partitioned plans allocating a dynamic set of keys across instance, persistent, and temporary storage tiers:

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn storage_manager_partitions_keys_properly(
        plan in strategies::mixed_tier_allocations("[a-z0-9_]{3,12}", 10..=30),
    ) {
        let (inst_count, pers_count, temp_count) = plan.counts();
        assert_eq!(inst_count + pers_count + temp_count, plan.len());

        for key in plan.temporary_keys() {
            // Test temporary key eviction / purging logic
        }
        for key in plan.persistent_keys() {
            // Test archive and restoration logic
        }
    }
}
```

## Host cost models and budget threshold fuzzing

Soroban meters contract execution across granular [`ContractCostType`](https://docs.rs/soroban-env-host/latest/soroban_env_host/xdr/enum.ContractCostType.html)
categories — including `WasmInsnExec`, `MemAlloc`, `MemCpy`, `MemCmp`, `ValSer`,
`ValDeser`, `ComputeSha256Hash`, `VerifyEd25519Sig`, `VmInstantiation`, and elliptic curve arithmetic.
Simulating resource cost fluctuations allows contracts to be stress-tested against
future protocol parameter adjustments and network fee shifts.

`contract_cost_model()` generates a realistic [`ContractCostModel`] covering all standard
`ContractCostType` enumerations. It generates valid host cost schedules (`ContractCostParams`)
directly compatible with `soroban-env-host`, supports configuring custom fee multipliers
for individual cost dimensions (`CostDimension::Cpu` and `CostDimension::Memory`), and
shrinks towards the baseline protocol fee schedule.

```rust
use proptest::prelude::*;
use soroban_env_host::xdr::ContractCostType;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn contract_execution_fits_under_fee_shifts(
        model in strategies::contract_cost_model(),
    ) {
        // Construct a soroban-env-host Budget loaded with the generated schedule
        let budget = model.try_into_budget(10_000_000, 10_000_000).unwrap();

        // Meter operations or invoke contract functions
        budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        budget.charge(ContractCostType::MemAlloc, Some(1024)).unwrap();

        assert!(budget.get_cpu_insns_remaining().unwrap() > 0);
    }
}
```

### Configuring fee multipliers

Use `contract_cost_model_with_multipliers` to supply percentage ranges for CPU and Memory
fee multipliers (where `100` represents 100% / 1.0x baseline):

```rust
use proptest::prelude::*;
use soroban_testkit_generators::strategies;

proptest! {
    #[test]
    fn test_tight_cpu_headroom(
        // Stress-test 1.5x - 4.0x CPU shifts and 1.0x - 2.0x Memory shifts
        model in strategies::contract_cost_model_with_multipliers(150..=400, 100..=200),
    ) {
        let (cpu_cost, mem_cost) = model.evaluate_cost(
            soroban_env_host::xdr::ContractCostType::ComputeSha256Hash,
            1,
            Some(256),
        );
        assert!(cpu_cost > 0);
        assert_eq!(mem_cost, 0);
    }
}
```

For fine-grained control, configure individual cost dimensions or specific `ContractCostType`
discriminants using `ContractCostModelConfig`:

```rust
use proptest::prelude::*;
use soroban_env_host::xdr::ContractCostType;
use soroban_testkit_generators::cost_model::{
    ContractCostModelConfig, CostDimension,
};
use soroban_testkit_generators::strategies;

let config = ContractCostModelConfig::new()
    .with_cpu_multiplier_range(100..=200)
    .with_mem_multiplier_range(100..=150)
    // Target a specific crypto primitive for severe cost surge
    .with_cost_type_multiplier_range(
        ContractCostType::VerifyEd25519Sig,
        CostDimension::Cpu,
        300..=1000,
    );

proptest! {
    #[test]
    fn ed25519_heavy_contract_tolerates_signature_cost_spike(
        model in strategies::contract_cost_model_with_config(config.clone()),
    ) {
        let budget = model.try_into_budget(5_000_000, 5_000_000).unwrap();
        budget.charge(ContractCostType::VerifyEd25519Sig, Some(64)).unwrap();
    }
}
```

<div class="tk-callout">
  <span class="tk-callout__title">Shrinking goes to the baseline protocol fee schedule</span>
  <p>proptest shrinks integer offsets towards zero. <code>contract_cost_model()</code> generates fee shifts as offsets relative to 100% baseline, so failing runs simplify towards the standard on-chain protocol fee schedule (<code>100%</code> / <code>1.0x</code>) rather than stopping at an arbitrary random multiplier.</p>
</div>

## XDR values: symbols, strings, bytes, vectors and maps

The `xdr` module generates the complex argument types contract interfaces
take, with configurable size bounds, and composes them into nested shapes.

```rust
use proptest::prelude::*;
use soroban_sdk::Env;
use soroban_testkit_generators::xdr::{xdr_bytes, xdr_map, xdr_string, xdr_symbol, xdr_vec};

let env = Env::default();

// Map<Symbol, Vec<Symbol>>: at most 5 keys, each with at most 4 symbols.
let tags = xdr_map(&env, xdr_symbol(&env, 10), xdr_vec(&env, xdr_symbol(&env, 10), 4), 5);

// A full argument tuple for `index(label: String, tags: Map<..>, payload: Bytes)`.
let args = (xdr_string(&env, 24), tags, xdr_bytes(&env, 32));
```

Each typed strategy returns a host value owned by `env`, so it can be passed
straight to a generated client. Element strategies can be any other strategy
whose value converts to and from `Val`, so `xdr_vec(&env, xdr_vec(&env,
xdr_symbol(&env, 8), 2), 3)` is a `Vec<Vec<Symbol>>`.

### Recursive values and depth limits

When a contract accepts a bare `Val` — or you want to fuzz any shape at all —
use `xdr_value` / `xdr_val` with an `XdrBounds`:

```rust
use soroban_testkit_generators::xdr::{xdr_val, xdr_value, XdrBounds};

let bounds = XdrBounds::default()       // symbol ≤ 32, string ≤ 64, bytes ≤ 64,
    .with_max_collection_len(4)         // vectors / maps ≤ 4 entries
    .with_max_depth(3);                 // at most 3 levels of nesting

proptest! {
    #[test]
    fn echo_round_trips(value in xdr_val(&env, bounds)) {
        let echoed = client.echo(&value);
        // compare via ScVal::try_from_val(&env, &value)
    }
}
```

| `XdrBounds` field | Default | Meaning |
|-------------------|---------|---------|
| `max_symbol_len` | 32 | longest symbol, clamped to the host's 32-character limit |
| `max_string_len` | 64 | longest string, in bytes |
| `max_bytes_len` | 64 | longest byte array |
| `max_collection_len` | 8 | most elements per vector / entries per map |
| `max_depth` | 3 | container nesting: `0` = scalars only, `1` = a vector or map of scalars, … |

`xdr_value` produces an `XdrValue` — an environment-free description covering
`Void`, `Bool`, `U32`, `I32`, `U64`, `I64`, `U128`, `I128`, `Symbol`, `String`,
`Bytes`, `Vec` and `Map` — that prints readably when a case fails and is turned
into a `Val` with `to_val(&env)`. Generation uses `prop_recursive`, so the
depth bound is a hard cap rather than a probability: a tree can never recurse
past `max_depth`, which keeps generation off the stack-overflow path and inside
the host's object limits. `XdrValue::depth()` and `max_collection_len()` let a
test assert the bounds held. Map keys are scalars and are de-duplicated, and
every generated value converts to a valid `ScVal`.

`tests/xdr_contract.rs` drives a sample contract that takes `String`,
`Map<Symbol, Vec<Symbol>>`, `Bytes`, `Val` and `Vec<Val>` arguments with these
strategies and checks every value round-trips through storage and across the
contract boundary.

## Fuzzing with `arbitrary` (cargo-fuzz, afl)

Coverage-guided fuzzers such as [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)
and [afl.rs](https://github.com/rust-fuzz/afl.rs) build typed inputs with the
[`arbitrary`](https://docs.rs/arbitrary) crate rather than proptest. The
`arbitrary` feature implements `Arbitrary` for the same value ranges the
proptest strategies use. It compiles on its own — proptest is not required:

```toml
[dependencies]
soroban-testkit-generators = { version = "0.4", default-features = false, features = ["arbitrary"] }
```

| Type | Generated value |
|------|-----------------|
| `AddressBytes` | an account (`G...`) or contract (`C...`) address, 32 arbitrary bytes |
| `AddressType` | `Account` or `Contract` |
| `TokenAmount` | `i128` in `0..=TOKEN_AMOUNT_MAX` (10^15) |
| `LedgerSequence` | `u32` in `1..=10_000_000` |
| `Timestamp` | `u64` in `1_600_000_000..=2_000_000_000` |
| `ArbitraryLedgerInfo` | a `LedgerInfo` the test host accepts (see below) |

The range constants (`TOKEN_AMOUNT_MAX`, `LEDGER_SEQUENCE_MIN/MAX`,
`TIMESTAMP_MIN/MAX`) live in the always-compiled `types` module and back both
the proptest strategies and the `Arbitrary` implementations, so the two
ecosystems can never disagree on what is valid. The bounded newtypes expose
`get()`, `MIN`, `MAX` and a checked `new(value)`.

`ArbitraryLedgerInfo` always reports protocol version 28, a valid sequence and
timestamp, minimum temporary and persistent TTLs of at least one ledger, a
`max_entry_ttl` no smaller than either minimum, and a sequence that cannot
overflow when the maximum TTL is added. Install it with `info.apply(&env)`.

**Generation never panics.** Every implementation draws with
`Unstructured::int_in_range` or fixed-size byte copies, which fall back to the
low end of the range when the fuzzer's input runs out. An empty, truncated or
all-`0xFF` input still produces a valid value.

### A cargo-fuzz harness

```rust
#![no_main]
use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use soroban_sdk::Env;
use soroban_testkit_generators::arbitrary::{AddressBytes, ArbitraryLedgerInfo, TokenAmount};

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let ledger = ArbitraryLedgerInfo::arbitrary(&mut u).unwrap();
    let payer = AddressBytes::arbitrary(&mut u).unwrap();
    let amount = TokenAmount::arbitrary(&mut u).unwrap();

    let env = Env::default();
    ledger.apply(&env);
    let payer = payer.to_address(&env);
    // ... call the contract under test with `payer` and `amount.get()` ...
});
```

The crate ships this as a ready-made project in
`crates/soroban-testkit-generators/fuzz/`, kept outside the workspace because
libFuzzer needs nightly:

```bash
cargo install cargo-fuzz
cd crates/soroban-testkit-generators
cargo +nightly fuzz run arbitrary_values
```

The target's body lives in `fuzz/fuzz_targets/harness.rs`, and the stable
integration test `tests/arbitrary_fuzz_harness.rs` runs the same harness over
empty, short, saturated and pseudo-random inputs, so `cargo test --workspace`
checks the "never panics, always valid" guarantee on every CI run.

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
