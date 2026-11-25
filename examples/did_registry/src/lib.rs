#![no_std]

//! A decentralized identifier (DID) and verifiable credential registry.
//!
//! The contract keeps one W3C-shaped DID Document per identifier and a status
//! record per credential, so a verifier can ask this ledger three questions
//! without trusting the holder: who controls a DID, where its services point,
//! and whether a credential has been revoked.
//!
//! # Compatibility boundaries
//!
//! The data structures use the field names and shapes of the W3C DID Core and
//! Verifiable Credentials data models, but this contract is not a conforming
//! implementation of either specification:
//!
//! - There is no JSON-LD processing, `@context` handling or canonicalisation.
//!   Documents are typed contract values, and a client is responsible for
//!   rendering them as JSON-LD if it needs one.
//! - Verification methods carry no `publicKeyMultibase` or JWK. Soroban cannot
//!   check a secp256k1 or ed25519 key encoded in another format, so the key
//!   material is *implied* by the controller: a Stellar account address is its
//!   ed25519 public key, and a contract address is its contract id.
//! - The registry does not verify credential proofs. A credential's `issuer` is
//!   the address that called `register_credential`, and revocation is an
//!   authoritative on-chain status, not a status list the issuer publishes
//!   elsewhere.
//! - DID resolution is a direct storage read: no resolution metadata, no
//!   `alsoKnownAs`, no deactivation. Only the fields below exist.

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, symbol_short, Address, Env, String,
    Symbol, Vec,
};

/// The only DID method this registry resolves. A DID is `did:stellar:<id>`.
pub const DID_METHOD_PREFIX: &str = "did:stellar:";

/// Longest DID identifier accepted, in bytes.
const MAX_DID_LEN: u32 = 128;

/// Longest method-specific identifier accepted, in bytes. A Stellar address is
/// 56 characters, so the cap leaves room for a sub-identifier without letting
/// one key reach a ledger entry limit.
const MAX_METHOD_ID_LEN: u32 = 64;

/// Longest verification method id accepted, in bytes.
const MAX_VERIFICATION_METHOD_LEN: u32 = 160;

/// A DID document holds at most this many service endpoints. The cap is what
/// keeps one document's storage predictable.
const MAX_SERVICES: u32 = 8;

/// Longest service endpoint id, in bytes. A service id is usually the document
/// DID plus a fragment, so the cap has to leave room for the largest DID the
/// registry accepts.
const MAX_SERVICE_ID_LEN: u32 = 200;

/// Longest service endpoint type, in bytes.
const MAX_SERVICE_TYPE_LEN: u32 = 32;

/// Longest service endpoint URI, in bytes.
const MAX_ENDPOINT_LEN: u32 = 200;

/// Longest credential identifier accepted, in bytes.
const MAX_CREDENTIAL_ID_LEN: u32 = 128;

/// One verification method of a DID document.
///
/// W3C DID Core also allows `publicKeyJwk`, `publicKeyMultibase` and a
/// `blockchainAccountId`. This contract stores the controller address instead,
/// which is the only key representation the Soroban host can act on.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationMethod {
    /// `<did>#<fragment>`, checked to start with the document's DID.
    pub id: String,
    /// The account or contract address whose signature changes the document.
    pub controller: Address,
}

/// One service endpoint of a DID document.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceEndpoint {
    /// `<did>#<fragment>` or any fragment the controller chooses.
    pub id: String,
    /// The service type, such as `LinkedDomains` or `Messaging`.
    pub service_type: String,
    /// An `https://` or `ipfs://` URI.
    pub endpoint: String,
}

/// One registered DID document.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DidDocument {
    pub id: String,
    /// The address that controls this document today.
    pub controller: Address,
    pub verification_method: VerificationMethod,
    pub services: Vec<ServiceEndpoint>,
    /// Starts at 1 and increases with every accepted modification.
    pub version: u32,
    /// The ledger timestamp the document was last written at.
    pub updated_at: u64,
}

/// The status record for one credential.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credential {
    pub id: String,
    /// The address that registered the credential.
    pub issuer: Address,
    /// The DID the credential was issued to.
    pub subject: String,
    pub issued_at: u64,
    pub revoked: bool,
    /// `0` until the credential is revoked.
    pub revoked_at: u64,
}

/// A DID document was registered for the first time.
#[contractevent(topics = ["did", "registered"])]
#[derive(Clone)]
pub struct DidRegistered {
    pub did: String,
    pub controller: Address,
}

/// The controller of a DID document moved to a new address.
#[contractevent(topics = ["did", "rotated"])]
#[derive(Clone)]
pub struct ControllerRotated {
    pub did: String,
    pub previous: Address,
    pub controller: Address,
}

/// The whole service endpoint list of a DID document was replaced.
#[contractevent(topics = ["did", "services"])]
#[derive(Clone)]
pub struct ServiceEndpointsUpdated {
    pub did: String,
    pub endpoints: u32,
}

/// A credential was registered by its issuer.
#[contractevent(topics = ["credential", "registered"])]
#[derive(Clone)]
pub struct CredentialRegistered {
    pub credential_id: String,
    pub issuer: Address,
    pub subject: String,
}

/// A credential was revoked by its issuer.
#[contractevent(topics = ["credential", "revoked"])]
#[derive(Clone)]
pub struct CredentialRevoked {
    pub credential_id: String,
    pub issuer: Address,
    pub revoked_at: u64,
}

fn admin_key() -> Symbol {
    symbol_short!("admin")
}

/// `(did, <identifier>)`, so every document is one persistent entry.
fn did_key(did: &String) -> (Symbol, String) {
    (symbol_short!("did"), did.clone())
}

/// `(cred, <identifier>)`, so every credential is one persistent entry.
fn credential_key(credential_id: &String) -> (Symbol, String) {
    (symbol_short!("cred"), credential_id.clone())
}

fn read_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&admin_key())
        .expect("registry not initialized")
}

fn read_did(env: &Env, did: &String) -> Option<DidDocument> {
    env.storage().persistent().get(&did_key(did))
}

fn write_did(env: &Env, did: &String, document: &DidDocument) {
    env.storage().persistent().set(&did_key(did), document);
}

fn read_credential(env: &Env, credential_id: &String) -> Option<Credential> {
    env.storage()
        .persistent()
        .get(&credential_key(credential_id))
}

fn write_credential(env: &Env, credential_id: &String, credential: &Credential) {
    env.storage()
        .persistent()
        .set(&credential_key(credential_id), credential);
}

/// Whether `value` begins with `prefix`, byte for byte.
fn starts_with(value: &String, prefix: &[u8]) -> bool {
    let bytes = value.to_bytes();
    if bytes.len() < prefix.len() as u32 {
        return false;
    }
    for (index, expected) in prefix.iter().enumerate() {
        if bytes.get(index as u32) != Some(*expected) {
            return false;
        }
    }
    true
}

/// Whether `value` begins with another host `String`.
fn starts_with_string(value: &String, prefix: &String) -> bool {
    let value = value.to_bytes();
    let prefix = prefix.to_bytes();
    if value.len() < prefix.len() {
        return false;
    }
    for index in 0..prefix.len() {
        if value.get(index) != prefix.get(index) {
            return false;
        }
    }
    true
}

/// Why an identifier is rejected, as a sentence a failing test prints unchanged.
fn check_did(did: &String) -> Result<(), &'static str> {
    let bytes = did.to_bytes();
    if bytes.len() > MAX_DID_LEN {
        return Err("DID is longer than 128 bytes");
    }
    if !starts_with(did, DID_METHOD_PREFIX.as_bytes()) {
        return Err("DID must use the did:stellar: method");
    }

    let prefix_len = DID_METHOD_PREFIX.len() as u32;
    let method_len = bytes.len() - prefix_len;
    if method_len == 0 {
        return Err("DID must name a method-specific identifier");
    }
    if method_len > MAX_METHOD_ID_LEN {
        return Err("DID method-specific identifier is longer than 64 bytes");
    }
    for index in prefix_len..bytes.len() {
        let byte = bytes.get(index).unwrap();
        let allowed = byte.is_ascii_alphanumeric()
            || byte == b'-'
            || byte == b'.'
            || byte == b'_'
            || byte == b':';
        if !allowed {
            return Err("DID may only contain letters, digits, '-', '.', '_' and ':'");
        }
    }
    Ok(())
}

fn validate_did(did: &String) {
    if let Err(message) = check_did(did) {
        panic!("{}", message);
    }
}

fn validate_verification_method(did: &String, method_id: &String) {
    let bytes = method_id.to_bytes();
    if bytes.len() > MAX_VERIFICATION_METHOD_LEN {
        panic!("verification method id is longer than 160 bytes");
    }
    if !starts_with_string(method_id, did) {
        panic!("verification method id must start with the DID");
    }
    if bytes.get(did.to_bytes().len()) != Some(b'#') {
        panic!("verification method id must be the DID followed by '#' and a fragment");
    }
}

fn validate_credential_id(credential_id: &String) {
    let bytes = credential_id.to_bytes();
    if bytes.is_empty() {
        panic!("credential id must not be empty");
    }
    if bytes.len() > MAX_CREDENTIAL_ID_LEN {
        panic!("credential id is longer than 128 bytes");
    }
    for index in 0..bytes.len() {
        let byte = bytes.get(index).unwrap();
        let allowed = byte.is_ascii_alphanumeric()
            || byte == b'-'
            || byte == b'.'
            || byte == b'_'
            || byte == b':';
        if !allowed {
            panic!("credential id may only contain letters, digits, '-', '.', '_' and ':'");
        }
    }
}

fn validate_services(services: &Vec<ServiceEndpoint>) {
    if services.len() > MAX_SERVICES {
        panic!("a DID document may hold at most 8 service endpoints");
    }

    for index in 0..services.len() {
        let service = services.get(index).unwrap();

        let id = service.id.to_bytes();
        if id.is_empty() || id.len() > MAX_SERVICE_ID_LEN {
            panic!("service endpoint id must be 1 to 200 bytes");
        }
        let kind = service.service_type.to_bytes();
        if kind.is_empty() || kind.len() > MAX_SERVICE_TYPE_LEN {
            panic!("service endpoint type must be 1 to 32 bytes");
        }
        let endpoint = service.endpoint.to_bytes();
        if endpoint.is_empty() || endpoint.len() > MAX_ENDPOINT_LEN {
            panic!("service endpoint URI must be 1 to 200 bytes");
        }
        if !(starts_with(&service.endpoint, b"https://")
            || starts_with(&service.endpoint, b"ipfs://"))
        {
            panic!("service endpoint URI must be an https:// or ipfs:// URI");
        }

        // A duplicate fragment id would make the endpoint list ambiguous, so a
        // list that repeats one is refused rather than silently de-duplicated.
        for earlier in 0..index {
            if services.get(earlier).unwrap().id == service.id {
                panic!("service endpoint ids must be unique within a document");
            }
        }
    }
}

#[contract]
pub struct DidRegistry;

#[contractimpl]
impl DidRegistry {
    /// Records the registry operator. Call it once, before any registration.
    ///
    /// The operator is not consulted by any other function: it exists so the
    /// registry has an identifiable owner and a deployment that never ran
    /// `initialize` fails loudly instead of silently accepting documents.
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        if env.storage().instance().has(&admin_key()) {
            panic!("registry already initialized");
        }
        env.storage().instance().set(&admin_key(), &admin);
    }

    /// The operator recorded by `initialize`.
    pub fn admin(env: Env) -> Address {
        read_admin(&env)
    }

    /// Registers a new DID document under `did`, controlled by `controller`.
    ///
    /// `method_id` must be `<did>#<fragment>`; the fragment is the controller's
    /// choice, and the key material is the controller address itself.
    pub fn register_did(
        env: Env,
        controller: Address,
        did: String,
        method_id: String,
        services: Vec<ServiceEndpoint>,
    ) -> DidDocument {
        controller.require_auth();
        validate_did(&did);
        validate_verification_method(&did, &method_id);
        validate_services(&services);

        if read_did(&env, &did).is_some() {
            panic!("DID is already registered");
        }

        let document = DidDocument {
            id: did.clone(),
            controller: controller.clone(),
            verification_method: VerificationMethod {
                id: method_id,
                controller: controller.clone(),
            },
            services,
            version: 1,
            updated_at: env.ledger().timestamp(),
        };
        write_did(&env, &did, &document);

        DidRegistered { did, controller }.publish(&env);

        document
    }

    /// The document registered for `did`, if any. A missing DID resolves to
    /// `None` rather than failing: asking about a DID that was never registered
    /// is an ordinary outcome of resolution.
    pub fn resolve_did(env: Env, did: String) -> Option<DidDocument> {
        read_did(&env, &did)
    }

    /// The address that controls `did` today.
    pub fn controller_of(env: Env, did: String) -> Option<Address> {
        read_did(&env, &did).map(|document| document.controller)
    }

    /// The current version of `did`'s document; `None` when it is unregistered.
    pub fn document_version(env: Env, did: String) -> Option<u32> {
        read_did(&env, &did).map(|document| document.version)
    }

    /// Moves control of `did` from `current_controller` to `new_controller`.
    ///
    /// The verification method id, the services and the DID itself stay put:
    /// rotation replaces the key material a method points at, which in this
    /// representation is the controller address.
    pub fn rotate_controller(
        env: Env,
        did: String,
        current_controller: Address,
        new_controller: Address,
    ) -> DidDocument {
        current_controller.require_auth();

        let mut document = read_did(&env, &did).expect("DID is not registered");
        if document.controller != current_controller {
            panic!("caller is not the DID controller");
        }
        if new_controller == current_controller {
            panic!("controller rotation must change the controller");
        }

        document.controller = new_controller.clone();
        document.verification_method.controller = new_controller.clone();
        document.version += 1;
        document.updated_at = env.ledger().timestamp();
        write_did(&env, &did, &document);

        ControllerRotated {
            did,
            previous: current_controller,
            controller: new_controller,
        }
        .publish(&env);

        document
    }

    /// Replaces the whole service endpoint list of `did`.
    ///
    /// Replacing rather than appending keeps the document one storage write and
    /// makes "this endpoint is gone" expressible; a caller that wants to add
    /// one endpoint submits the list that includes it.
    pub fn set_service_endpoints(
        env: Env,
        did: String,
        controller: Address,
        services: Vec<ServiceEndpoint>,
    ) -> DidDocument {
        controller.require_auth();
        validate_services(&services);

        let mut document = read_did(&env, &did).expect("DID is not registered");
        if document.controller != controller {
            panic!("caller is not the DID controller");
        }

        document.services = services;
        document.version += 1;
        document.updated_at = env.ledger().timestamp();
        write_did(&env, &did, &document);

        ServiceEndpointsUpdated {
            did,
            endpoints: document.services.len(),
        }
        .publish(&env);

        document
    }

    /// The service endpoint with `service_id`, if the document holds one.
    pub fn service_endpoint(env: Env, did: String, service_id: String) -> Option<ServiceEndpoint> {
        let document = read_did(&env, &did)?;
        for index in 0..document.services.len() {
            let service = document.services.get(index).unwrap();
            if service.id == service_id {
                return Some(service);
            }
        }
        None
    }

    /// Registers `credential_id` as issued by `issuer` to `subject`.
    ///
    /// The subject DID must already be registered. The registry records status
    /// only: it does not inspect or verify the credential's proof.
    pub fn register_credential(
        env: Env,
        issuer: Address,
        credential_id: String,
        subject: String,
    ) -> Credential {
        issuer.require_auth();
        validate_credential_id(&credential_id);
        validate_did(&subject);

        if read_did(&env, &subject).is_none() {
            panic!("subject DID is not registered");
        }
        if read_credential(&env, &credential_id).is_some() {
            panic!("credential is already registered");
        }

        let credential = Credential {
            id: credential_id.clone(),
            issuer: issuer.clone(),
            subject: subject.clone(),
            issued_at: env.ledger().timestamp(),
            revoked: false,
            revoked_at: 0,
        };
        write_credential(&env, &credential_id, &credential);

        CredentialRegistered {
            credential_id,
            issuer,
            subject,
        }
        .publish(&env);

        credential
    }

    /// Revokes `credential_id`. Only the issuer that registered it may do so,
    /// and a credential is revoked at most once.
    pub fn revoke_credential(env: Env, issuer: Address, credential_id: String) -> Credential {
        issuer.require_auth();

        let mut credential =
            read_credential(&env, &credential_id).expect("credential is not registered");
        if credential.issuer != issuer {
            panic!("caller is not the credential issuer");
        }
        if credential.revoked {
            panic!("credential is already revoked");
        }

        credential.revoked = true;
        credential.revoked_at = env.ledger().timestamp();
        write_credential(&env, &credential_id, &credential);

        CredentialRevoked {
            credential_id,
            issuer,
            revoked_at: credential.revoked_at,
        }
        .publish(&env);

        credential
    }

    /// The status record for `credential_id`, if it is registered.
    pub fn credential(env: Env, credential_id: String) -> Option<Credential> {
        read_credential(&env, &credential_id)
    }

    /// Whether `credential_id` is revoked.
    ///
    /// `None` means the registry has never seen the credential, which a
    /// verifier must treat differently from `Some(false)`: an unknown
    /// credential is not a valid one.
    pub fn is_revoked(env: Env, credential_id: String) -> Option<bool> {
        read_credential(&env, &credential_id).map(|credential| credential.revoked)
    }
}

#[cfg(test)]
mod test;
