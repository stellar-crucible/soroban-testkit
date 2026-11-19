#![no_std]

//! A decentralized identifier (DID) registry and verifiable credential
//! revocation list, written as a worked example for the Soroban testkit.
//!
//! The registry stores a deliberately small subset of the W3C DID Core data
//! model: an identifier, one controller, Ed25519 verification methods,
//! authentication relationships that name those methods, and service
//! endpoints. It does **not** implement the DID resolution algorithm, DID
//! methods beyond the illustrative `did:soroban:` prefix, key encodings other
//! than 32 raw Ed25519 bytes, or verifiable credential issuance and
//! verification. Credentials are represented only by the identifier an issuer
//! revokes.
//!
//! Every document lives in one persistent map keyed by its identifier, and
//! every modification is authorized by the document's current controller. A
//! controller change rewrites the controller of both the document and each of
//! its verification methods, so an address that was a controller a moment ago
//! can no longer authorize anything.

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, Address, BytesN, Env, Map, String, Symbol,
    Vec,
};

/// Longest DID identifier the registry accepts, in bytes.
pub const MAX_DID_LEN: u32 = 96;

/// Longest verification method or service endpoint id, in bytes.
pub const MAX_ID_LEN: u32 = 64;

/// Longest service type, in bytes.
pub const MAX_SERVICE_TYPE_LEN: u32 = 64;

/// Longest service endpoint URI, in bytes.
pub const MAX_ENDPOINT_LEN: u32 = 256;

/// Longest credential revocation reason, in bytes.
pub const MAX_REASON_LEN: u32 = 128;

/// Longest credential identifier, in bytes.
pub const MAX_CREDENTIAL_ID_LEN: u32 = 128;

/// Verification methods one document may carry.
pub const MAX_VERIFICATION_METHODS: u32 = 8;

/// Authentication relationships one document may carry.
pub const MAX_AUTHENTICATION: u32 = 8;

/// Service endpoints one document may carry.
pub const MAX_SERVICES: u32 = 8;

/// The identifier prefix this registry understands.
///
/// `soroban` is an illustrative method name, not a registered DID method: the
/// prefix is validated so identifiers have the W3C `did:method:id` shape, and
/// nothing else is claimed about it.
const DID_PREFIX: &[u8] = b"did:soroban:";

/// One Ed25519 verification method.
///
/// `id` is a DID URL fragment such as `#key-1`, unique inside its document.
/// `public_key` is the raw 32-byte Ed25519 key; the registry does not accept
/// other curves or encoded key material.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationMethod {
    pub id: String,
    pub controller: Address,
    pub public_key: BytesN<32>,
}

/// One service endpoint, the registry's subset of a DID Core `service` entry.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceEndpoint {
    pub id: String,
    pub service_type: String,
    pub endpoint: String,
}

/// The DID document subset the registry stores.
///
/// `authentication` holds verification method ids rather than duplicated key
/// material, so rotating a key leaves the relationship in place and pointing at
/// the new key. `services` is a vector because the document is always written
/// as a whole and order is preserved between reads.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DidDocument {
    pub id: String,
    pub controller: Address,
    pub verification_methods: Vec<VerificationMethod>,
    pub authentication: Vec<String>,
    pub services: Vec<ServiceEndpoint>,
}

/// A credential the issuer's controller has revoked.
///
/// The record is first-write-wins: revoking the same credential again returns
/// the original `revoked_at` and reason instead of rewriting them.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevocationRecord {
    pub credential_id: String,
    pub issuer: String,
    pub revoked_at: u64,
    pub reason: String,
}

/// A DID was registered.
#[contractevent(topics = ["did", "registered"])]
#[derive(Clone)]
pub struct Registered {
    pub did: String,
    pub controller: Address,
}

/// A verification method's public key was replaced.
#[contractevent(topics = ["did", "key_rotated"])]
#[derive(Clone)]
pub struct VerificationKeyRotated {
    pub did: String,
    pub key_id: String,
}

/// A document moved to a new controller.
#[contractevent(topics = ["did", "controller_rotated"])]
#[derive(Clone)]
pub struct ControllerRotated {
    pub did: String,
    pub from: Address,
    pub to: Address,
}

/// A service endpoint was added.
#[contractevent(topics = ["did", "service_added"])]
#[derive(Clone)]
pub struct ServiceAdded {
    pub did: String,
    pub service_id: String,
}

/// A service endpoint was replaced.
#[contractevent(topics = ["did", "service_updated"])]
#[derive(Clone)]
pub struct ServiceUpdated {
    pub did: String,
    pub service_id: String,
}

/// A service endpoint was removed.
#[contractevent(topics = ["did", "service_removed"])]
#[derive(Clone)]
pub struct ServiceRemoved {
    pub did: String,
    pub service_id: String,
}

/// A credential was revoked, or its already-recorded revocation was asked for
/// again.
#[contractevent(topics = ["credential", "revoked"])]
#[derive(Clone)]
pub struct CredentialRevoked {
    pub credential_id: String,
    pub issuer: String,
}

fn documents_key(env: &Env) -> Symbol {
    Symbol::new(env, "documents")
}

fn revocations_key(env: &Env) -> Symbol {
    Symbol::new(env, "revoked")
}

/// Every registered document, in one persistent map.
fn documents(env: &Env) -> Map<String, DidDocument> {
    env.storage()
        .persistent()
        .get(&documents_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_documents(env: &Env, map: &Map<String, DidDocument>) {
    env.storage().persistent().set(&documents_key(env), map);
}

/// Every revoked credential, in one persistent map.
fn revocations(env: &Env) -> Map<String, RevocationRecord> {
    env.storage()
        .persistent()
        .get(&revocations_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_revocations(env: &Env, map: &Map<String, RevocationRecord>) {
    env.storage().persistent().set(&revocations_key(env), map);
}

fn fail(message: &'static str) -> ! {
    panic!("{}", message)
}

fn check_required(condition: bool, message: &'static str) {
    if !condition {
        fail(message);
    }
}

/// Every identifier character the registry accepts: lower-case ASCII letters,
/// digits, `-`, `.` and `_`.
fn allowed_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || byte == b'-'
        || byte == b'.'
        || byte == b'_'
}

/// Every id character the registry accepts for a verification method or a
/// service endpoint, which allows the W3C fragment form `#key-1`.
fn allowed_id_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || byte == b'-'
        || byte == b'.'
        || byte == b'_'
        || byte == b':'
        || byte == b'#'
}

/// Why a DID identifier is rejected, as a sentence a test prints unchanged.
fn check_did(did: &String) -> Result<(), &'static str> {
    let bytes = did.to_bytes();
    let len = bytes.len();
    if len == 0 || len > MAX_DID_LEN {
        return Err("DID identifier must be 1 to 96 bytes");
    }
    if len <= DID_PREFIX.len() as u32 {
        return Err("DID identifier must name something after did:soroban:");
    }
    for index in 0..DID_PREFIX.len() as u32 {
        if bytes.get(index).unwrap() != DID_PREFIX[index as usize] {
            return Err("DID identifier must start with did:soroban:");
        }
    }
    for index in DID_PREFIX.len() as u32..len {
        if !allowed_identifier_byte(bytes.get(index).unwrap()) {
            return Err("DID identifier may only use a-z, 0-9, '-', '.' and '_'");
        }
    }
    Ok(())
}

fn validate_did(did: &String) {
    if let Err(message) = check_did(did) {
        fail(message);
    }
}

fn check_id(value: &String, field: &'static str, empty: &'static str) -> Result<(), &'static str> {
    let bytes = value.to_bytes();
    let len = bytes.len();
    if len == 0 || len > MAX_ID_LEN {
        return Err(empty);
    }
    for index in 0..len {
        if !allowed_id_byte(bytes.get(index).unwrap()) {
            return Err(field);
        }
    }
    Ok(())
}

fn validate_method(method: &VerificationMethod, controller: &Address) {
    if let Err(message) = check_id(
        &method.id,
        "verification method id may only use a-z, A-Z, 0-9, '-', '.', '_', ':' and '#'",
        "verification method id must be 1 to 64 bytes",
    ) {
        fail(message);
    }
    if &method.controller != controller {
        fail("verification methods must be controlled by the DID controller");
    }
}

fn validate_service(service: &ServiceEndpoint) {
    if let Err(message) = check_id(
        &service.id,
        "service endpoint id may only use a-z, A-Z, 0-9, '-', '.', '_', ':' and '#'",
        "service endpoint id must be 1 to 64 bytes",
    ) {
        fail(message);
    }

    let service_type = service.service_type.to_bytes();
    if service_type.is_empty() || service_type.len() > MAX_SERVICE_TYPE_LEN {
        fail("service type must be 1 to 64 bytes");
    }
    for index in 0..service_type.len() {
        let byte = service_type.get(index).unwrap();
        let allowed = byte.is_ascii_alphanumeric()
            || byte == b'-'
            || byte == b'.'
            || byte == b'_'
            || byte == b'/';
        if !allowed {
            fail("service type may only use a-z, A-Z, 0-9, '-', '.', '_' and '/'");
        }
    }

    let endpoint = service.endpoint.to_bytes();
    if endpoint.is_empty() || endpoint.len() > MAX_ENDPOINT_LEN {
        fail("service endpoint must be 1 to 256 bytes");
    }
    let https = prefix_matches(&endpoint, b"https://");
    let ipfs = prefix_matches(&endpoint, b"ipfs://");
    if !https && !ipfs {
        fail("service endpoint must be an https:// or ipfs:// URI");
    }
}

fn prefix_matches(value: &soroban_sdk::Bytes, prefix: &[u8]) -> bool {
    if value.len() < prefix.len() as u32 {
        return false;
    }
    for index in 0..prefix.len() as u32 {
        if value.get(index).unwrap() != prefix[index as usize] {
            return false;
        }
    }
    true
}

fn has_method(methods: &Vec<VerificationMethod>, id: &String) -> bool {
    for index in 0..methods.len() {
        if &methods.get(index).unwrap().id == id {
            return true;
        }
    }
    false
}

fn service_index(services: &Vec<ServiceEndpoint>, id: &String) -> Option<u32> {
    (0..services.len()).find(|index| &services.get(*index).unwrap().id == id)
}

/// Checks every invariant of a stored document.
///
/// Both registration and every later modification run this after applying the
/// change, so no path can leave a document that authenticates a key it does not
/// carry, carries a duplicate id, or exceeds a size cap.
fn validate_document(document: &DidDocument) {
    validate_did(&document.id);

    check_required(
        !document.verification_methods.is_empty(),
        "DID document needs at least one verification method",
    );
    check_required(
        document.verification_methods.len() <= MAX_VERIFICATION_METHODS,
        "DID document is limited to 8 verification methods",
    );
    check_required(
        !document.authentication.is_empty(),
        "DID document needs at least one authentication relationship",
    );
    check_required(
        document.authentication.len() <= MAX_AUTHENTICATION,
        "DID document is limited to 8 authentication relationships",
    );
    check_required(
        document.services.len() <= MAX_SERVICES,
        "DID document is limited to 8 service endpoints",
    );

    for index in 0..document.verification_methods.len() {
        let method = document.verification_methods.get(index).unwrap();
        validate_method(&method, &document.controller);
        for earlier in 0..index {
            if document.verification_methods.get(earlier).unwrap().id == method.id {
                fail("DID document has a duplicate verification method id");
            }
        }
    }

    for index in 0..document.authentication.len() {
        let id = document.authentication.get(index).unwrap();
        if !has_method(&document.verification_methods, &id) {
            fail("authentication must reference a verification method of this document");
        }
        for earlier in 0..index {
            if document.authentication.get(earlier).unwrap() == id {
                fail("authentication lists the same verification method twice");
            }
        }
    }

    for index in 0..document.services.len() {
        let service = document.services.get(index).unwrap();
        validate_service(&service);
        for earlier in 0..index {
            if document.services.get(earlier).unwrap().id == service.id {
                fail("DID document has a duplicate service endpoint id");
            }
        }
    }
}

fn validate_credential_id(credential_id: &String) {
    let bytes = credential_id.to_bytes();
    if bytes.is_empty() || bytes.len() > MAX_CREDENTIAL_ID_LEN {
        fail("credential id must be 1 to 128 bytes");
    }
    for index in 0..bytes.len() {
        let byte = bytes.get(index).unwrap();
        let allowed = byte.is_ascii_alphanumeric()
            || byte == b'-'
            || byte == b'.'
            || byte == b'_'
            || byte == b':';
        if !allowed {
            fail("credential id may only use a-z, A-Z, 0-9, '-', '.', '_' and ':'");
        }
    }
}

/// Fails unless `signer` is the document's controller.
///
/// `require_auth` proves the controller's address signed; this proves the
/// signer is still *the* controller. Both are needed: after a controller change
/// the old address can still produce a valid signature, and this is the check
/// that turns it away.
fn require_controller(document: &DidDocument, signer: &Address, message: &'static str) {
    if &document.controller != signer {
        fail(message);
    }
}

/// Loads a document or fails with a stable sentence.
fn load_document(map: &Map<String, DidDocument>, did: &String) -> DidDocument {
    match map.get(did.clone()) {
        Some(document) => document,
        None => fail("DID is not registered"),
    }
}

#[contract]
pub struct DidRegistry;

#[contractimpl]
impl DidRegistry {
    /// Registers a DID document for `controller`.
    ///
    /// The document must name the same address as its controller, carry at
    /// least one verification method and authentication relationship, and every
    /// verification method must be controlled by that address. Registration is
    /// permissionless: an address registers its own identifier, and no operator
    /// pre-approves it.
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

        Registered {
            did: document.id.clone(),
            controller: document.controller.clone(),
        }
        .publish(&env);

        document
    }

    /// The document registered for `did`, or `None`.
    pub fn resolve(env: Env, did: String) -> Option<DidDocument> {
        documents(&env).get(did)
    }

    /// Whether `did` has a document.
    pub fn is_registered(env: Env, did: String) -> bool {
        documents(&env).contains_key(did)
    }

    /// How many DIDs are registered.
    pub fn document_count(env: Env) -> u32 {
        documents(&env).len()
    }

    /// The controller recorded for `did`, or `None`.
    ///
    /// Reading the controller alone is the check a relying party makes before it
    /// asks for an authorization; it does not need the whole document.
    pub fn controller_of(env: Env, did: String) -> Option<Address> {
        documents(&env).get(did).map(|document| document.controller)
    }

    /// Replaces the public key of an existing verification method.
    ///
    /// The method stays at its position and keeps its id and controller, so
    /// authentication relationships remain valid and now resolve to the new key.
    pub fn rotate_verification_key(
        env: Env,
        controller: Address,
        did: String,
        key_id: String,
        public_key: BytesN<32>,
    ) -> DidDocument {
        controller.require_auth();

        let mut all = documents(&env);
        let mut document = load_document(&all, &did);
        require_controller(
            &document,
            &controller,
            "only the DID controller can rotate a verification key",
        );

        let mut methods = document.verification_methods.clone();
        let mut found = false;
        for index in 0..methods.len() {
            let mut method = methods.get(index).unwrap();
            if method.id == key_id {
                method.public_key = public_key.clone();
                methods.set(index, method);
                found = true;
            }
        }
        if !found {
            fail("verification method is not part of this DID document");
        }

        document.verification_methods = methods;
        validate_document(&document);
        all.set(did.clone(), document.clone());
        write_documents(&env, &all);

        VerificationKeyRotated {
            did: did.clone(),
            key_id,
        }
        .publish(&env);

        document
    }

    /// Moves a document to `new_controller`.
    ///
    /// Every verification method moves with it, because a method naming the old
    /// controller would contradict the document it belongs to. After this call
    /// the previous controller authorizes nothing: each mutating entry point
    /// requires the recorded controller, not merely a signature.
    pub fn transfer_controller(
        env: Env,
        controller: Address,
        did: String,
        new_controller: Address,
    ) -> DidDocument {
        controller.require_auth();

        let mut all = documents(&env);
        let mut document = load_document(&all, &did);
        require_controller(
            &document,
            &controller,
            "only the DID controller can transfer control of a DID",
        );
        if new_controller == controller {
            fail("new controller must differ from the current controller");
        }

        let mut methods = document.verification_methods.clone();
        for index in 0..methods.len() {
            let mut method = methods.get(index).unwrap();
            method.controller = new_controller.clone();
            methods.set(index, method);
        }
        document.verification_methods = methods;

        let previous = document.controller.clone();
        document.controller = new_controller.clone();
        validate_document(&document);
        all.set(did.clone(), document.clone());
        write_documents(&env, &all);

        ControllerRotated {
            did,
            from: previous,
            to: new_controller,
        }
        .publish(&env);

        document
    }

    /// Adds a service endpoint to a document.
    pub fn add_service(
        env: Env,
        controller: Address,
        did: String,
        service: ServiceEndpoint,
    ) -> DidDocument {
        controller.require_auth();

        let mut all = documents(&env);
        let mut document = load_document(&all, &did);
        require_controller(
            &document,
            &controller,
            "only the DID controller can add a service endpoint",
        );
        validate_service(&service);
        if service_index(&document.services, &service.id).is_some() {
            fail("service endpoint is already registered for this DID");
        }
        check_required(
            document.services.len() < MAX_SERVICES,
            "DID document is limited to 8 service endpoints",
        );

        document.services.push_back(service.clone());
        validate_document(&document);
        all.set(did.clone(), document.clone());
        write_documents(&env, &all);

        ServiceAdded {
            did,
            service_id: service.id,
        }
        .publish(&env);

        document
    }

    /// Replaces the service endpoint with the same id, leaving every other
    /// field of the document untouched.
    pub fn update_service(
        env: Env,
        controller: Address,
        did: String,
        service: ServiceEndpoint,
    ) -> DidDocument {
        controller.require_auth();

        let mut all = documents(&env);
        let mut document = load_document(&all, &did);
        require_controller(
            &document,
            &controller,
            "only the DID controller can update a service endpoint",
        );
        validate_service(&service);

        let Some(index) = service_index(&document.services, &service.id) else {
            fail("service endpoint is not registered for this DID");
        };
        document.services.set(index, service.clone());
        validate_document(&document);
        all.set(did.clone(), document.clone());
        write_documents(&env, &all);

        ServiceUpdated {
            did,
            service_id: service.id,
        }
        .publish(&env);

        document
    }

    /// Removes a service endpoint by id.
    ///
    /// Removing an id the document does not carry is refused rather than
    /// treated as a no-op, so a client that misspells an id does not believe a
    /// live endpoint is gone.
    pub fn remove_service(
        env: Env,
        controller: Address,
        did: String,
        service_id: String,
    ) -> DidDocument {
        controller.require_auth();

        let mut all = documents(&env);
        let mut document = load_document(&all, &did);
        require_controller(
            &document,
            &controller,
            "only the DID controller can remove a service endpoint",
        );

        let Some(index) = service_index(&document.services, &service_id) else {
            fail("service endpoint is not registered for this DID");
        };
        document.services.remove(index);
        validate_document(&document);
        all.set(did.clone(), document.clone());

        write_documents(&env, &all);

        ServiceRemoved { did, service_id }.publish(&env);

        document
    }

    /// Revokes the credential `credential_id` on behalf of `issuer`.
    ///
    /// `issuer` must be a registered DID and `controller` its recorded
    /// controller, which is what stops one issuer revoking another issuer's
    /// credentials. Revocation is final and idempotent: a second call for the
    /// same credential returns the original record, its original timestamp and
    /// reason, and publishes nothing.
    pub fn revoke_credential(
        env: Env,
        controller: Address,
        issuer: String,
        credential_id: String,
        reason: String,
    ) -> RevocationRecord {
        controller.require_auth();
        validate_credential_id(&credential_id);
        if reason.len() > MAX_REASON_LEN {
            fail("revocation reason is capped at 128 bytes");
        }

        let mut revoked = revocations(&env);
        if let Some(existing) = revoked.get(credential_id.clone()) {
            if existing.issuer != issuer {
                fail("credential is already revoked by another issuer");
            }
            let issuer_document = load_document(&documents(&env), &existing.issuer);
            require_controller(
                &issuer_document,
                &controller,
                "only the issuer DID controller can revoke a credential",
            );
            return existing;
        }

        let issuer_document = load_document(&documents(&env), &issuer);
        require_controller(
            &issuer_document,
            &controller,
            "only the issuer DID controller can revoke a credential",
        );

        let record = RevocationRecord {
            credential_id: credential_id.clone(),
            issuer,
            revoked_at: env.ledger().timestamp(),
            reason,
        };
        revoked.set(credential_id, record.clone());
        write_revocations(&env, &revoked);

        CredentialRevoked {
            credential_id: record.credential_id.clone(),
            issuer: record.issuer.clone(),
        }
        .publish(&env);

        record
    }

    /// The revocation record for `credential_id`, or `None` while it is valid.
    pub fn revocation_status(env: Env, credential_id: String) -> Option<RevocationRecord> {
        revocations(&env).get(credential_id)
    }

    /// Whether `credential_id` has been revoked.
    pub fn is_revoked(env: Env, credential_id: String) -> bool {
        revocations(&env).contains_key(credential_id)
    }

    /// How many credentials have been revoked.
    pub fn revocation_count(env: Env) -> u32 {
        revocations(&env).len()
    }
}
