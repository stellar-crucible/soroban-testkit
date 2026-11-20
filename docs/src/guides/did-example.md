# Worked Example: DID Registry

A registry for decentralized identifiers (DIDs) and verifiable credential status. It is the first example whose subject is identity rather than money: every state change is gated on a controller's authorization, the stored value is a DID Document shaped like the W3C data model, and the question a caller asks most often is a yes/no status check rather than a balance.

Everything here is real code in [`examples/did_registry`](https://github.com/stellar-crucible/soroban-testkit/tree/main/examples/did_registry), a workspace member you can run.

<div class="tk-spec">
  <div class="tk-spec__item"><span class="tk-spec__key">Source</span><span class="tk-spec__value">examples/did_registry</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Crates</span><span class="tk-spec__value">fixtures · assert · core · generators</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Tests</span><span class="tk-spec__value">45 passing</span></div>
  <div class="tk-spec__item"><span class="tk-spec__key">Run</span><span class="tk-spec__value">cargo test -p soroban-testkit-example-did-registry</span></div>
</div>

## What this example demonstrates

1. **A registry where authorization is the whole security model.** Every state change calls `require_auth`; rotation and endpoint updates additionally compare the signer with the stored controller, so a mocked-auth test cannot accidentally pass an unauthorized modification.
2. **W3C-shaped data under Soroban's serialization constraints.** A DID Document with `id`, `controller`, a verification method and service endpoints, stored as typed contract values instead of JSON-LD.
3. **A deterministic credential revocation registry.** An issuer records a credential, and later revokes it exactly once; a verifier asks the ledger whether it is revoked, and gets `None` for a credential the registry has never seen.
4. **All four testkit crates**, one per file, including a property test for the central claim: only an authorized controller can modify a DID document.

## DID Document structure and supported fields

The contract defines four `#[contracttype]` values. Field names follow the W3C DID Core model, lower-cased to Rust conventions.

```rust
pub struct DidDocument {
    pub id: String,                            // did:stellar:<identifier>
    pub controller: Address,                   // the address that controls it
    pub verification_method: VerificationMethod,
    pub services: Vec<ServiceEndpoint>,
    pub version: u32,                          // 1 on registration, +1 per change
    pub updated_at: u64,                       // ledger timestamp of the last write
}

pub struct VerificationMethod {
    pub id: String,                            // <did>#<fragment>
    pub controller: Address,                   // key material, in Stellar form
}

pub struct ServiceEndpoint {
    pub id: String,                            // <did>#<fragment>
    pub service_type: String,                  // e.g. "LinkedDomains"
    pub endpoint: String,                      // https:// or ipfs:// URI
}
```

Validation is deliberate and bounded, so one document cannot grow without limit:

| Field | Rule |
| --- | --- |
| `DidDocument.id` | `did:stellar:` followed by 1–64 bytes of `A–Z a–z 0–9 - . _ :`; at most 128 bytes total |
| `VerificationMethod.id` | must start with the document DID, then `#` and a fragment; at most 160 bytes |
| `services` | at most 8 entries |
| `ServiceEndpoint.id` | 1–200 bytes, unique within the document |
| `ServiceEndpoint.service_type` | 1–32 bytes |
| `ServiceEndpoint.endpoint` | 1–200 bytes and an `https://` or `ipfs://` URI |

**A compatibility boundary worth stating plainly:** these are W3C-*shaped* values, not a conforming DID Core implementation. There is no `@context`, no JSON-LD canonicalisation, no `publicKeyMultibase`/JWK, no `alsoKnownAs`, no deactivation and no resolution metadata. The verification method carries a **controller address** instead of encoded key material: a Stellar account address is its ed25519 public key, and a contract address is its contract id, so the host can enforce the method without ever parsing a foreign key format.

## Contract registration and initialization

`initialize` records the registry operator once. It is not consulted by any other entry point; it exists so a deployment that never ran initialization fails loudly (`admin()` panics with `registry not initialized`) instead of silently accepting documents.

```rust
let env = Env::default();
env.mock_all_auths();
let contract = env.register(DidRegistry, ());
let admin = Address::generate(&env);
let client = DidRegistryClient::new(&env, &contract);

client.initialize(&admin);
assert_eq!(client.admin(), admin);
```

## Registering a DID

```rust
pub fn register_did(
    env: Env,
    controller: Address,
    did: String,
    method_id: String,
    services: Vec<ServiceEndpoint>,
) -> DidDocument
```

`controller.require_auth()` runs first. Then the identifier, the verification method id and every endpoint are validated, and only then is the document read to see whether the DID exists. A second registration of the same DID panics with `DID is already registered`; control moves by rotation, which the current controller authorizes, never by re-registration.

```rust
let controller = Address::generate(&env);
let did = String::from_str(&env, "did:stellar:alice");
let method = String::from_str(&env, "did:stellar:alice#key-1");

let document = client.register_did(&controller, &did, &method, &Vec::new(&env));
assert_eq!(document.version, 1);
assert_eq!(document.controller, controller);
assert_eq!(document.verification_method.controller, controller);
```

The document is stored under one persistent key — `(symbol_short!("did"), did)` — so a registration writes one entry and never rewrites a shared map.

## Resolving or inspecting a DID document

```rust
let resolved = client.resolve_did(&did);          // Option<DidDocument>
let owner = client.controller_of(&did);           // Option<Address>
let version = client.document_version(&did);      // Option<u32>
let endpoint = client.service_endpoint(&did, &fragment); // Option<ServiceEndpoint>
```

A missing DID resolves to `None`. That is an ordinary resolution outcome, not an error, which is why the read does not panic and why the property tests can assert "the DID still resolves to exactly what was registered" after a refused write.

## Rotating a controller key

```rust
pub fn rotate_controller(
    env: Env,
    did: String,
    current_controller: Address,
    new_controller: Address,
) -> DidDocument
```

Rotation is the DID equivalent of key rotation: the verification method id, the services and the identifier stay put, and the address the method points at changes. Three refusals are explicit:

- the DID must be registered (`DID is not registered`);
- `current_controller` must be the stored controller (`caller is not the DID controller`) — this is what a mocked-auth test can check;
- the target must differ from the current controller (`controller rotation must change the controller`), so a "rotation" cannot silently be a no-op that bumps the version.

An accepted rotation increments `version` and `updated_at`, rewrites the same storage entry, and publishes a `rotated` event naming both addresses.

```rust
let rotated = client.rotate_controller(&did, &controller, &new_controller);
assert_eq!(rotated.controller, new_controller);
assert_eq!(rotated.verification_method.controller, new_controller);
assert_eq!(rotated.version, 2);
```

With mock auths disabled, a caller that is not the controller fails at `require_auth` before any state is read.

## Updating service endpoints

```rust
pub fn set_service_endpoints(
    env: Env,
    did: String,
    controller: Address,
    services: Vec<ServiceEndpoint>,
) -> DidDocument
```

The call **replaces** the whole list rather than appending. That makes "this endpoint is gone" expressible in one deterministic write, keeps the document one storage entry, and means the test can assert the count equals what the controller submitted. Registration with a non-empty list is how endpoints are first created; this function is how they are updated.

```rust
let services = Vec::from_array(&env, [ServiceEndpoint {
    id: String::from_str(&env, "did:stellar:alice#domain"),
    service_type: String::from_str(&env, "LinkedDomains"),
    endpoint: String::from_str(&env, "https://alice.example"),
}]);
let updated = client.set_service_endpoints(&did, &controller, &services);
assert_eq!(updated.services.len(), 1);
assert_eq!(updated.version, 3);
```

Endpoint data is validated before the document is read, so a bad URI is refused on an unregistered DID too. Duplicate fragment ids, an unsupported scheme (`ftp://`), an empty type or a ninth endpoint all panic with a sentence the test prints unchanged.

## Registering and revoking credentials

```rust
pub struct Credential {
    pub id: String,
    pub issuer: Address,
    pub subject: String,   // a registered DID
    pub issued_at: u64,
    pub revoked: bool,
    pub revoked_at: u64,   // 0 until revoked
}
```

```rust
client.register_credential(&issuer, &credential_id, &subject_did);
client.revoke_credential(&issuer, &credential_id);
```

Registration requires the issuer's authorization, a credential id of 1–128 bytes (`A–Z a–z 0–9 - . _ :`), a structurally valid subject DID, and that the subject DID is already registered. Credential ids are global to the registry, so a duplicate is refused rather than treated as a second credential.

Revocation requires the issuer that registered the credential and happens at most once. A repeated revocation panics with `credential is already revoked` and leaves the original `revoked_at` timestamp untouched — an auditor reading the record sees when it was *first* revoked, not when someone retried.

<div class="tk-callout">
  <span class="tk-callout__title">What the registry does not verify</span>
  <p>No credential proof is inspected. The issuer is the address that called <code>register_credential</code>, and revocation is an authoritative on-chain status, not a status list fetched from the issuer. A contract cannot check an off-chain VC signature, so claiming "verifiable credential conformance" here would be false; what the example implements is the <em>status registry</em> half.</p>
</div>

## Checking credential revocation status

```rust
let status = client.credential(&credential_id);   // Option<Credential>
let revoked = client.is_revoked(&credential_id);  // Option<bool>
```

Three states, deliberately distinct:

| Result | Meaning |
| --- | --- |
| `Some(true)` | registered and revoked; `revoked_at` says when |
| `Some(false)` | registered and currently valid |
| `None` | the registry has never seen this credential |

A verifier must treat `None` as "unknown", not as "valid" — collapsing the two is the bug the property test `an_unknown_credential_is_reported_as_unknown_not_as_active` exists to prevent.

## Running the example's tests

```bash
cargo test -p soroban-testkit-example-did-registry
```

```text
running 15 tests   # src/test.rs — fixtures
test result: ok. 15 passed
running 10 tests   # tests/assert_events.rs — assertions
test result: ok. 10 passed
running 11 tests   # tests/budget_and_storage.rs — core
test result: ok. 11 passed
running 9 tests    # tests/property.rs — generators
test result: ok. 9 passed
```

45 tests in the package, all inside `cargo test --workspace --locked`. The suite is split by crate, one file each:

| File | Crate | What it covers |
| --- | --- | --- |
| `src/test.rs` | `soroban-testkit-fixtures` | `TestContextBuilder`, `user(index)`, `add_user`, `reset`, and the contract's validation rules |
| `tests/assert_events.rs` | `soroban-testkit-assert` | `EventMatcher`, `EventLog`, `assert_auth_matches!`, `AuthMatcher`, and one unmocked authorization refusal |
| `tests/budget_and_storage.rs` | `soroban-testkit-core` | `budget_guard!`, `BudgetSnapshot`, `BudgetBaseline`, `StorageSnapshot`, `FootprintSnapshot`, `assert_storage_tier!` |
| `tests/property.rs` | `soroban-testkit-generators` | `proptest!` over account/contract controllers, documents and endpoint lists |

The property test that carries the security claim generates an authorized controller, an attacker, a rotation target, a modification kind (`rotate`, replace endpoints, clear endpoints) and an endpoint count. It asserts the attacker's operation is refused **and** that `resolve_did`, `controller_of` and `document_version` still return the original document; then it performs the change as the controller and asserts the version moved exactly once.

## Understanding storage and budget measurements

`tests/budget_and_storage.rs` measures two different things.

**Storage footprint** uses `FootprintSnapshot`, which reports XDR-encoded bytes — the unit rent is charged in. Each measurement deploys and initializes the registry, captures the total, registers one document, and subtracts:

```rust
let before = FootprintSnapshot::capture(&env, &contract).total_bytes();
client(&env, &contract).register_did(&controller, &did, &method, &endpoints);
let after = FootprintSnapshot::capture(&env, &contract).total_bytes();
let document_bytes = after - before;   // one persistent key plus its value
```

Measured on this workspace, with the document sizes the test documents:

| Document | Services | URI bytes each | Bytes added |
| --- | --- | --- | --- |
| small | 0 | — | 452 |
| medium | 2 | 32 | 772 |
| large | 8 (the cap) | 168 | 2820 |

The test measures each size twice and asserts the second reading equals the first: the numbers are XDR sizes rather than heap readings, so they are reproducible.

**Budget** uses `BudgetSnapshot::last_invocation` and `BudgetBaseline`, measuring the four state-changing paths on a document with two endpoints:

```text
   register_did cpu_insns=108684 mem_bytes=13837
rotate_controller cpu_insns=112211 mem_bytes=18518
register_credential cpu_insns=102649 mem_bytes=14825
 revoke_credential cpu_insns=72421  mem_bytes=12521
```

Those lines are printed by `a_baseline_survives_the_file_ci_commits` on every run (`-- --nocapture` to see them) and are serialized through `BudgetBaseline::to_json_string`, which is the format a committed baseline file uses. They are measurements of a contract registered natively with `env.register`, so VM instantiation, wasm execution and rent reads are not metered: compare builds of this contract against each other, and deploy a wasm build when a fee quote is what you need.

`budget_guard!` supplies the ceilings used in CI:

```rust
soroban_testkit_core::budget_guard!(
    &env,
    "register_did",
    { cpu_max: 20_000_000, mem_max: 20_000_000 },
    || client(&env, &contract).register_did(&controller, &did, &method, &Vec::new(&env))
);
```

A breach renders one parseable line, pinned by a `#[should_panic(expected = "BUDGET kind=ceiling case=register_did metric=cpu_insns")]` test.

## Security assumptions and limitations

- **The controller is the key.** Authorization is `require_auth` on a Stellar address plus an equality check against the stored controller. Soroban's account abstraction is doing the signature checking; this contract never sees a public key.
- **No credential proof verification.** A credential's existence on chain means the issuer registered it, not that a presentation is valid. Verifiers must still check issuer trust and any off-chain proof.
- **Credential ids are global.** Two issuers cannot register the same id, and there is no per-issuer namespace or revocation list. A production system would namespace ids or store one status list per issuer.
- **Rotation does not invalidate history.** The version increments, but earlier credentials remain valid and earlier signatures over the old document are not recorded anywhere. There is no key-history or `deactivated` flag.
- **No enumeration.** Documents and credentials are keyed entries; there is no index and no way to list them. A client must know the DID or credential id it is asking about.
- **Persistent entries need rent management.** The contract never calls `extend_ttl`, so a document or credential whose entry expires is archived and resolves as missing until it is restored. An operator that wants documents to outlive the default TTL has to bump entries from a maintenance call.
- **DID method scope.** `did:stellar:` is a method this example defines for itself; it is not a registered DID method, and no external resolver will know it. The `controller` embedded in a document is not cross-checked against the method-specific identifier, so a DID may name one address and be controlled by another — which is legal in DID Core, and deliberate here.
- **The operator has no special powers.** `admin` is informational; there is no pause, no forced revocation and no upgrade path in this contract.
