# Worked Example: DID and Verifiable Credential Registry

[`examples/did_registry`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/did_registry)
is a permissionless Decentralized Identifier (DID) registry with a credential
revocation list. It is the example for identity-shaped work: state that is owned
by an address rather than an operator, authorization that changes hands, and
documents whose size is part of their cost.

Its suite uses all four testkit crates, one per file:

| File | Crate | Question it answers |
|------|-------|---------------------|
| `tests/fixtures.rs` | `soroban-testkit-fixtures` | What does a valid document look like, and which fixtures are malformed? |
| `tests/assert.rs` | `soroban-testkit-assert` | What did the contract emit and record as authorized, and which error did it raise? |
| `tests/budget_storage.rs` | `soroban-testkit-core` | What does an operation cost, in instructions and in ledger bytes? |
| `tests/proptest.rs` | `soroban-testkit-generators` | Does authorization hold for inputs nobody wrote by hand? |

Shared builders live in `tests/common/mod.rs`. Cargo compiles every
`tests/*.rs` as its own crate, so a helper cannot be imported from a sibling
test file; the module next to them is the standard way to share one, and all
four files pull it in with `mod common;`.

## Running the example

```bash
cargo test -p soroban-testkit-example-did-registry
cargo test -p soroban-testkit-example-did-registry -- --nocapture   # measurements
cargo test -p soroban-testkit-example-did-registry --test proptest
```

The package has 55 tests: 8 fixture cases, 21 assertion cases, 14 measurement
cases and 12 property tests (each running 32 generated cases).

## What it demonstrates

- Registering a DID document with a controller's authorization.
- Duplicate registration is refused and leaves the first document intact.
- Resolving a document, and resolving one that does not exist.
- Rotating a verification method's public key.
- Moving a document to a new controller, after which the old controller can no
  longer modify anything.
- Adding, updating and removing service endpoints while preserving the rest of
  the document.
- Revoking a credential as its issuer's controller, querying revocation status,
  and the idempotence of a repeated revocation.

## The supported DID Document subset

The contract deliberately models a small subset of
[W3C DID Core](https://www.w3.org/TR/did-core/):

```rust
#[contracttype]
pub struct VerificationMethod {
    pub id: String,              // a fragment such as "#key-1"
    pub controller: Address,
    pub public_key: BytesN<32>,  // raw Ed25519 key material
}

#[contracttype]
pub struct ServiceEndpoint {
    pub id: String,
    pub service_type: String,   // e.g. "LinkedDomains"
    pub endpoint: String,       // an https:// or ipfs:// URI
}

#[contracttype]
pub struct DidDocument {
    pub id: String,                           // did:soroban:<name>
    pub controller: Address,
    pub verification_methods: Vec<VerificationMethod>,
    pub authentication: Vec<String>,          // ids of methods above
    pub services: Vec<ServiceEndpoint>,
}
```

What is supported:

- A single controller per document, as an ed25519 account or a contract address.
- Ed25519 verification methods, 32 raw bytes, with `controller` matching the
  document's controller.
- Authentication relationships that name verification method ids, so rotating a
  key leaves the relationship pointing at the same method.
- Service endpoints with a type and an `https://` or `ipfs://` URI.

What is **not** implemented, and is not claimed:

- DID resolution as the W3C defines it (the contract's `resolve` is a storage
  read; there is no universal resolver, no representation negotiation and no
  DID Resolution metadata).
- A registered DID method. `did:soroban:` is an illustrative prefix; the
  registry only validates its shape so identifiers look like `did:method:id`.
- Key formats other than Ed25519, key encodings such as multibase or JWK, and
  `alsoKnownAs`, delegation, or multiple controllers.
- Verifiable credential issuance or verification. A credential exists here only
  as the identifier an issuer revokes.

## Registration and resolution

Registration is permissionless: an address registers its own identifier and no
operator pre-approves it. The controller must authorize the call, and the
document must name that same address.

```rust
pub fn register(env: Env, controller: Address, document: DidDocument) -> DidDocument {
    controller.require_auth();
    validate_document(&document);
    if document.controller != controller {
        fail("document controller must authorize its own registration");
    }

    let mut all = documents(&env);
    if all.contains_key(document.id.clone()) {
        fail("DID is already registered");
    }
    all.set(document.id.clone(), document.clone());
    write_documents(&env, &all);
    // ... publish Registered { did, controller }
    document
}
```

`validate_document` is the single invariant gate. It requires at least one
verification method and one authentication relationship, caps a document at
eight of each, rejects duplicate method or service ids, rejects an
authentication relationship that names a method the document does not carry,
and rejects a verification method naming another controller. It runs on
registration and again after every modification, so no path can leave a
document in a state registration would have refused.

Resolution is a plain read, and an unknown identifier answers `None` rather
than failing:

```rust
pub fn resolve(env: Env, did: String) -> Option<DidDocument> {
    documents(&env).get(did)
}
```

Duplicate registration raises `"DID is already registered"` and leaves the
first document untouched.

## Controller authorization and key rotation

Soroban's `require_auth` proves that an address signed. It does not prove that
the address is still the document's controller. After a controller change the
old address can still produce a valid signature, so every mutating entry point
does both checks:

```rust
fn require_controller(document: &DidDocument, signer: &Address, message: &'static str) {
    if &document.controller != signer {
        fail(message);
    }
}
```

`rotate_verification_key` replaces the 32-byte key of an existing method. The
method keeps its id, position and controller, so authentication relationships
keep pointing at it and now resolve to the new key:

```rust
client.rotate_verification_key(&controller, &did, &text(&env, "#key-1"), &new_key);
```

`transfer_controller` moves the document and rewrites the controller of every
verification method with it, because a method naming the old controller would
contradict its document. The previous controller authorizes nothing afterwards:

```rust
let moved = client.transfer_controller(&controller, &did, &successor);
assert_eq!(moved.controller, successor);
for index in 0..moved.verification_methods.len() {
    assert_eq!(moved.verification_methods.get(index).unwrap().controller, successor);
}
```

The assertion-based test drives the refusal through the real entry points:

```rust
assert!(client.try_add_service(&controller, &did, &endpoint).is_err());
assert!(client.try_rotate_verification_key(&controller, &did, &key_id, &key).is_err());
client.add_service(&successor, &did, &endpoint);   // the new controller succeeds
```

The property test `only_the_recorded_controller_can_add_a_service` generates a
signer and a controller, registers, then asserts the call succeeds exactly when
the two addresses are equal — and that a refused call left the document
unchanged. It exercises the contract, not a helper.

## Service endpoint management

`add_service`, `update_service` and `remove_service` each authorize the
controller, validate the endpoint, and re-validate the document. An endpoint
looks like this:

```rust
let endpoint = ServiceEndpoint {
    id: text(&env, "svc-1"),
    service_type: text(&env, "LinkedDomains"),
    endpoint: text(&env, "https://alice.example"),
};
client.add_service(&controller, &did, &endpoint);
```

Adding an id the document already carries, updating or removing one it does not
carry, a duplicate id, an empty id, a type over 64 bytes, an endpoint over 256
bytes, and an endpoint that is not `https://` or `ipfs://` are all refused.
Modifications preserve everything else: the tests compare the verification
methods, the authentication relationships and the untouched endpoints before and
after each call.

## Credential revocation

A credential is revoked by the controller of its issuer's registered DID, not by
whoever asks:

```rust
pub fn revoke_credential(
    env: Env,
    controller: Address,
    issuer: String,
    credential_id: String,
    reason: String,
) -> RevocationRecord {
    controller.require_auth();
    // ... validate the credential id and reason, then:
    if let Some(existing) = revoked.get(credential_id.clone()) {
        // first write wins; re-authorize against the recorded issuer
        if existing.issuer != issuer {
            fail("credential is already revoked by another issuer");
        }
        // ... require_controller(issuer document), then return `existing`
    }
    // ... require the issuer's recorded controller, store, publish
}
```

Status queries are public reads:

```rust
client.revocation_status(&credential);   // Option<RevocationRecord>
client.is_revoked(&credential);          // bool
client.revocation_count();               // u32
```

Revocation is final and idempotent. A second call for the same credential
returns the original record, timestamp and reason, writes nothing and publishes
no second event. A credential cannot be reassigned to a different issuer after
it has been revoked.

## How the four crates are used

**Fixtures** (`tests/fixtures.rs`) supplies `TestContextBuilder` and
`TestContext`: one call produces an environment, an admin and named users, and
`ctx.env.register(DidRegistry, ())` deploys the contract. The file's tests are
about the fixtures themselves — that the small, medium and large document
builders register, that each malformed fixture is refused, and that every
invalid identifier in the fixture list fails in a fresh environment.

**Assertions** (`tests/assert.rs`) uses `EventMatcher` and `EventLog` to read
the `registered`, `key_rotated`, `controller_rotated`, `service_added`,
`service_updated`, `service_removed` and `revoked` events and their data;
`assert_auth_matches!` and `AuthMatcher` to check who authorized which function
and that reads require nothing. A matcher reads the invocation that just ran, so
each test asserts its event before a read replaces the scope, and `EventLog`
collects several calls when the sequence matters.

**Core** (`tests/budget_storage.rs`) uses `StorageSnapshot` to diff entries,
`FootprintSnapshot` for XDR byte sizes, `BudgetSnapshot::last_invocation` for
per-call instructions and memory, `budget_guard!` / `BudgetGuard` for ceilings,
and `BudgetBaseline` for the JSON round trip CI commits.

**Generators** (`tests/proptest.rs`) uses `account_address`,
`contract_address` and `timestamp` from
`soroban-testkit-generators::strategies` to drive the authorization properties
and to cover both address formats, plus regex-generated identifiers and
credentials for boundary cases.

## Measurements

`tests/budget_storage.rs` prints its measurements; the assertions check
*relationships*, not absolute numbers, so an SDK bump does not turn the file
red. Run it with `-- --nocapture` to see them. The numbers below were recorded
on 2026-11-19 with Rust 1.100.0 and `soroban-sdk` 28 on this repository:

| Document shape | `documents` XDR bytes | rendered value bytes |
|----------------|-----------------------|----------------------|
| 1 verification method | 516 | 570 |
| 2 verification methods | 1120 | 1433 |
| 4 verification methods | 2076 | 2882 |
| 8 verification methods | 3736 | 5503 |
| 0 service endpoints | 516 | — |
| 1 service endpoint | 1084 | — |
| 4 service endpoints | 2072 | — |
| 8 service endpoints | 3620 | — |

Registration cost for a one-method document and an eight-method,
eight-endpoint document:

| Case | CPU instructions | Memory bytes |
|------|------------------|--------------|
| small document | 68,586 | 8,901 |
| large document | 411,953 | 32,238 |

One revocation record renders as 243 bytes; two render as 486 bytes, because
revocations share one map entry instead of opening a key each.

The same one-method registration measured 68,826 CPU instructions and 8,904
memory bytes in one environment, and an identical run in a second fresh
environment measured exactly the same. Rewriting a stored document costs more
than the first write (106,373 instructions for a key rotation against 68,826
for registration) because it reads the map back and re-validates the document.

Methodology, in short:

- Byte sizes are the XDR length of the ledger entry (`FootprintSnapshot`), the
  unit rent is charged in. The whole `documents` map is one entry, so a
  measurement per document would need a key per document; this design trades
  that for a fixed storage shape.
- Costs are `BudgetSnapshot::last_invocation`, the top-level invocation that
  just ran. A natively registered test contract is metered without the VM's own
  costs, so these numbers are lower than a network would bill and are meaningful
  relatively, not absolutely.
- Every test builds its own `Env`, so a measurement cannot inherit another
  test's storage or metering.
- No absolute cost is asserted. The repository's committed baseline mechanism is
  demonstrated with `BudgetBaseline`, and `examples/counter/budget.json` is where
  the repository actually pins costs on one runner.

## Security assumptions and limitations

- **Authorization is the recorded controller.** `require_auth` plus the explicit
  controller equality check are both required; either alone would let a stale
  signer or an unauthorized address through.
- **The registry is permissionless.** Anyone can register any unused
  `did:soroban:` identifier. Availability of a human-readable name is not
  guaranteed and is not this contract's job.
- **One map, whole-document rewrites.** Every modification reads and rewrites
  the entire `documents` map, so per-call cost grows with the number of
  registered DIDs. The measurement suite makes that visible; a production
  registry would key each document separately.
- **Revocation is first-write-wins and permanent.** There is no un-revoke and no
  expiry. A credential id is an opaque string; the contract does not check that
  the credential exists or that the issuer ever issued it.
- **Revocation authority follows the DID.** If an issuer DID changes
  controllers, the new controller can revoke and can repeat an existing
  revocation; the old one cannot.
- **Bounded inputs.** Identifiers are capped at 96 bytes, verification methods
  at 8 per document (64-byte ids), service endpoints at 8 (64-byte type,
  256-byte URI), authentication relationships at 8, and reasons at 128 bytes.
  There is no arithmetic on user-controlled values beyond length comparisons, so
  overflow is not reachable.
- **`#![no_std]`.** The contract uses only `soroban-sdk` types; the test files
  are ordinary `std` tests.

## Differences from full W3C compliance

This is a worked example of contract and test patterns, not a DID
implementation. It validates the `did:method:id` shape but implements no
registered method; it stores a resolution result but not the resolution
algorithm; it supports one key type and one controller; it omits delegates,
`alsoKnownAs`, key encodings and representation metadata; and it records
credential revocation only, with no issuance, proof format or status list
structure. Treat the data model as an illustration of how identity state can be
owned and authorized on Soroban, not as a conformance claim.
