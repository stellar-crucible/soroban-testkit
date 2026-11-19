#![no_std]

use soroban_sdk::{
    contract, contractevent, contractimpl, contracttype, Address, Env, Map, String, Symbol,
};

/// A registration term is counted in whole years, and a year is 365 days. The
/// registry never reads wall time, so a test moves `env.ledger().timestamp()`
/// and the expiry follows.
pub const SECONDS_PER_YEAR: u64 = 365 * 24 * 60 * 60;

/// The shortest name the registry accepts: two characters would make the
/// namespace a squatter's market rather than a registry.
const MIN_NAME_LEN: u32 = 3;

/// Longest name the registry accepts, in bytes. The cap is what keeps one
/// domain key small enough to stay inside a ledger entry.
const MAX_NAME_LEN: u32 = 32;

/// Longest registration a single call may buy.
const MAX_TERM_YEARS: u32 = 10;

/// One registry entry: who holds the name and when the hold ends.
#[contracttype]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Domain {
    pub owner: Address,
    pub expires_at: u64,
}

/// The three record types a name can point at.
///
/// A resolver record is a typed string rather than free-form bytes: a Stellar
/// address, an IPFS content id and a text record are read by different clients,
/// and the kind is what tells them apart.
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordKind {
    /// A `G...` or `C...` address the name pays to.
    Stellar,
    /// An IPFS content identifier.
    Ipfs,
    /// Free text, such as a verification string.
    Text,
}

/// A name was registered for the first time, or registered again after its
/// grace period expired.
#[contractevent(topics = ["domain", "registered"])]
#[derive(Clone)]
pub struct Registered {
    pub name: String,
    pub owner: Address,
    pub expires_at: u64,
}

/// A term was extended. `expires_at` is the new end of the term.
#[contractevent(topics = ["domain", "renewed"])]
#[derive(Clone)]
pub struct Renewed {
    pub name: String,
    pub owner: Address,
    pub expires_at: u64,
}

/// Ownership moved. The term does not move with it.
#[contractevent(topics = ["domain", "transferred"])]
#[derive(Clone)]
pub struct Transferred {
    pub name: String,
    pub from: Address,
    pub to: Address,
}

/// A resolver record was written for a name.
#[contractevent(topics = ["domain", "record"])]
#[derive(Clone)]
pub struct RecordSet {
    pub name: String,
    pub kind: RecordKind,
    pub value: String,
}

/// An expired name was taken by a new owner after its grace period ran out.
#[contractevent(topics = ["domain", "claimed"])]
#[derive(Clone)]
pub struct Claimed {
    pub name: String,
    pub claimant: Address,
    pub expires_at: u64,
}

fn admin_key(env: &Env) -> Symbol {
    Symbol::new(env, "admin")
}

fn treasury_key(env: &Env) -> Symbol {
    Symbol::new(env, "treasury")
}

fn base_fee_key(env: &Env) -> Symbol {
    Symbol::new(env, "base_fee")
}

fn grace_period_key(env: &Env) -> Symbol {
    Symbol::new(env, "grace_period")
}

fn domains_key(env: &Env) -> Symbol {
    Symbol::new(env, "domains")
}

fn records_key(env: &Env) -> Symbol {
    Symbol::new(env, "records")
}

fn balances_key(env: &Env) -> Symbol {
    Symbol::new(env, "balances")
}

fn read_admin(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&admin_key(env))
        .expect("registry not initialized")
}

fn read_treasury(env: &Env) -> Address {
    env.storage()
        .instance()
        .get(&treasury_key(env))
        .expect("registry not initialized")
}

fn read_base_fee(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&base_fee_key(env))
        .expect("registry not initialized")
}

fn read_grace_period(env: &Env) -> u64 {
    env.storage()
        .instance()
        .get(&grace_period_key(env))
        .expect("registry not initialized")
}

/// Every registered name, in one persistent map.
///
/// One map rather than one key per name keeps the registry's storage shape
/// fixed: a registration rewrites one entry instead of opening a new one, which
/// is what the storage-growth test measures.
fn domains(env: &Env) -> Map<String, Domain> {
    env.storage()
        .persistent()
        .get(&domains_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_domains(env: &Env, map: &Map<String, Domain>) {
    env.storage().persistent().set(&domains_key(env), map);
}

/// A name's resolver records, keyed by record kind.
fn records(env: &Env) -> Map<String, Map<RecordKind, String>> {
    env.storage()
        .persistent()
        .get(&records_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_records(env: &Env, map: &Map<String, Map<RecordKind, String>>) {
    env.storage().persistent().set(&records_key(env), map);
}

/// The credits a holder has deposited, which is what renewal fees are paid
/// from. A registry without a token of its own keeps the ledger here.
fn balances(env: &Env) -> Map<Address, i128> {
    env.storage()
        .persistent()
        .get(&balances_key(env))
        .unwrap_or_else(|| Map::new(env))
}

fn write_balances(env: &Env, map: &Map<Address, i128>) {
    env.storage().persistent().set(&balances_key(env), map);
}

/// Why a name is rejected, as a sentence a failing test prints unchanged.
fn check_name(name: &String) -> Result<(), &'static str> {
    let bytes = name.to_bytes();
    let len = bytes.len();
    if !(MIN_NAME_LEN..=MAX_NAME_LEN).contains(&len) {
        return Err("domain name must be 3 to 32 characters");
    }
    for index in 0..len {
        let byte = bytes.get(index).unwrap();
        let allowed = byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-';
        if !allowed {
            return Err("domain name may only contain a-z, 0-9 and '-'");
        }
        let edge = index == 0 || index + 1 == len;
        if edge && !(byte.is_ascii_lowercase() || byte.is_ascii_digit()) {
            return Err("domain name must start and end with a letter or digit");
        }
    }
    Ok(())
}

fn validate_name(name: &String) {
    if let Err(message) = check_name(name) {
        panic!("{}", message);
    }
}

fn validate_term(years: u32) {
    if years == 0 || years > MAX_TERM_YEARS {
        panic!("registration term must be between 1 and 10 years");
    }
}

/// What `years` of registration costs, before any deposit is drawn down.
fn fee_for(env: &Env, years: u32) -> i128 {
    read_base_fee(env) * years as i128
}

/// Debits `from` and credits the treasury, or fails without moving anything.
fn charge(env: &Env, from: &Address, amount: i128) {
    let mut map = balances(env);
    let held = map.get(from.clone()).unwrap_or(0);
    if held < amount {
        panic!("insufficient balance for the registry fee");
    }
    map.set(from.clone(), held - amount);
    let treasury = read_treasury(env);
    let received = map.get(treasury.clone()).unwrap_or(0);
    map.set(treasury, received + amount);
    write_balances(env, &map);
}

#[contract]
pub struct DomainService;

#[contractimpl]
impl DomainService {
    /// Records the registry's operator, the address fees are paid to, the price
    /// of one year and the window in which an expired name still belongs to its
    /// previous owner. Call it once, before any registration.
    pub fn initialize(
        env: Env,
        admin: Address,
        treasury: Address,
        base_fee: i128,
        grace_period: u64,
    ) {
        admin.require_auth();
        if env.storage().instance().has(&admin_key(&env)) {
            panic!("registry already initialized");
        }
        if base_fee < 0 {
            panic!("base fee must not be negative");
        }

        env.storage().instance().set(&admin_key(&env), &admin);
        env.storage().instance().set(&treasury_key(&env), &treasury);
        env.storage().instance().set(&base_fee_key(&env), &base_fee);
        env.storage()
            .instance()
            .set(&grace_period_key(&env), &grace_period);
    }

    /// The operator recorded by `initialize`.
    pub fn admin(env: Env) -> Address {
        read_admin(&env)
    }

    /// The address every fee is credited to.
    pub fn treasury(env: Env) -> Address {
        read_treasury(&env)
    }

    /// What one year costs.
    pub fn base_fee(env: Env) -> i128 {
        read_base_fee(&env)
    }

    /// Seconds an expired name still belongs to its previous owner.
    pub fn grace_period(env: Env) -> u64 {
        read_grace_period(&env)
    }

    /// What a term of `years` costs. Linear, so a test can predict the charge
    /// rather than read it back.
    pub fn price(env: Env, years: u32) -> i128 {
        validate_term(years);
        fee_for(&env, years)
    }

    /// Whether `name` is a name the registry would accept, without needing a
    /// registered contract state. This is the view a client checks before it
    /// pays for a registration.
    pub fn is_valid_name(env: Env, name: String) -> bool {
        let _ = env;
        check_name(&name).is_ok()
    }

    /// The entry for `name`, whether or not it has expired.
    pub fn domain(env: Env, name: String) -> Option<Domain> {
        domains(&env).get(name)
    }

    /// The holder of `name`, whether or not the term has expired.
    pub fn owner_of(env: Env, name: String) -> Option<Address> {
        domains(&env).get(name).map(|domain| domain.owner)
    }

    /// When `name` was last paid through.
    pub fn expires_at(env: Env, name: String) -> Option<u64> {
        domains(&env).get(name).map(|domain| domain.expires_at)
    }

    /// Whether a new registration could take `name` right now.
    ///
    /// An expired name stays unavailable through its grace period, because the
    /// previous owner can still renew it; after that it is available again.
    pub fn is_available(env: Env, name: String) -> bool {
        match domains(&env).get(name) {
            None => true,
            Some(domain) => {
                let claimable_at = domain.expires_at.saturating_add(read_grace_period(&env));
                env.ledger().timestamp() >= claimable_at
            }
        }
    }

    /// Credits `from` with `amount` so renewals and registrations can be paid
    /// for. The registry has no token of its own, so this is its ledger.
    pub fn deposit(env: Env, from: Address, amount: i128) {
        from.require_auth();
        if amount <= 0 {
            panic!("deposit must be positive");
        }
        let mut map = balances(&env);
        let held = map.get(from.clone()).unwrap_or(0);
        map.set(from.clone(), held + amount);
        write_balances(&env, &map);
    }

    /// What `owner` has deposited and not yet spent.
    pub fn balance_of(env: Env, owner: Address) -> i128 {
        balances(&env).get(owner).unwrap_or(0)
    }

    /// Registers `name` to `owner` for `years`, charging the term's fee.
    ///
    /// A name whose grace period has run out is available again, and the
    /// registration clears whatever records the previous owner left behind.
    pub fn register(env: Env, owner: Address, name: String, years: u32) -> Domain {
        owner.require_auth();
        validate_term(years);
        validate_name(&name);

        let now = env.ledger().timestamp();
        let mut map = domains(&env);
        let replaced = map.get(name.clone());
        if let Some(existing) = &replaced {
            if existing.expires_at > now {
                panic!("domain is already registered");
            }
            if now < existing.expires_at.saturating_add(read_grace_period(&env)) {
                panic!("domain is inside its grace period; claim it instead");
            }
        }

        charge(&env, &owner, fee_for(&env, years));

        if replaced.is_some() {
            let mut record_map = records(&env);
            record_map.remove(name.clone());
            write_records(&env, &record_map);
        }

        let domain = Domain {
            owner,
            expires_at: now + years as u64 * SECONDS_PER_YEAR,
        };
        map.set(name.clone(), domain.clone());
        write_domains(&env, &map);

        Registered {
            name,
            owner: domain.owner.clone(),
            expires_at: domain.expires_at,
        }
        .publish(&env);

        domain
    }

    /// Extends `name` by `years`, starting from the end of the current term
    /// when that is still in the future and from now when it is not.
    ///
    /// Renewal is allowed through the grace period, which is what makes the
    /// grace period worth having: the previous owner can pay to keep the name
    /// without competing for it.
    pub fn renew(env: Env, owner: Address, name: String, years: u32) -> Domain {
        owner.require_auth();
        validate_term(years);
        validate_name(&name);

        let now = env.ledger().timestamp();
        let mut map = domains(&env);
        let existing = map.get(name.clone()).expect("domain is not registered");

        if existing.owner != owner {
            panic!("only the owner can renew this domain");
        }
        if now >= existing.expires_at.saturating_add(read_grace_period(&env)) {
            panic!("domain is past its grace period; claim it instead");
        }

        charge(&env, &owner, fee_for(&env, years));

        let start = if existing.expires_at > now {
            existing.expires_at
        } else {
            now
        };
        let renewed = Domain {
            owner,
            expires_at: start + years as u64 * SECONDS_PER_YEAR,
        };
        map.set(name.clone(), renewed.clone());
        write_domains(&env, &map);

        Renewed {
            name,
            owner: renewed.owner.clone(),
            expires_at: renewed.expires_at,
        }
        .publish(&env);

        renewed
    }

    /// Moves ownership of `name` to `to`. The term does not change.
    pub fn transfer(env: Env, from: Address, name: String, to: Address) {
        from.require_auth();
        validate_name(&name);

        let now = env.ledger().timestamp();
        let mut map = domains(&env);
        let existing = map.get(name.clone()).expect("domain is not registered");

        if existing.owner != from {
            panic!("only the owner can transfer this domain");
        }
        if existing.expires_at <= now {
            panic!("domain has expired");
        }

        map.set(
            name.clone(),
            Domain {
                owner: to.clone(),
                expires_at: existing.expires_at,
            },
        );
        write_domains(&env, &map);

        Transferred { name, from, to }.publish(&env);
    }

    /// Takes over a name whose grace period has run out, paying one year.
    ///
    /// The records the previous owner wrote go with the name: a new owner must
    /// not inherit a payment address or a content pointer it never set.
    pub fn claim(env: Env, claimant: Address, name: String) -> Domain {
        claimant.require_auth();
        validate_name(&name);

        let now = env.ledger().timestamp();
        let mut map = domains(&env);
        let existing = map.get(name.clone()).expect("domain is not registered");

        if now < existing.expires_at.saturating_add(read_grace_period(&env)) {
            panic!("domain is not claimable before its grace period ends");
        }

        charge(&env, &claimant, fee_for(&env, 1));

        let mut record_map = records(&env);
        record_map.remove(name.clone());
        write_records(&env, &record_map);

        let claimed = Domain {
            owner: claimant.clone(),
            expires_at: now + SECONDS_PER_YEAR,
        };
        map.set(name.clone(), claimed.clone());
        write_domains(&env, &map);

        Claimed {
            name,
            claimant,
            expires_at: claimed.expires_at,
        }
        .publish(&env);

        claimed
    }

    /// Writes one resolver record for a name the caller holds and that has not
    /// expired.
    pub fn set_record(env: Env, owner: Address, name: String, kind: RecordKind, value: String) {
        owner.require_auth();
        validate_name(&name);

        if value.len() > 256 {
            panic!("record value is capped at 256 bytes");
        }

        let now = env.ledger().timestamp();
        let existing = domains(&env)
            .get(name.clone())
            .expect("domain is not registered");
        if existing.owner != owner {
            panic!("only the owner can set a record");
        }
        if existing.expires_at <= now {
            panic!("domain has expired");
        }

        let mut record_map = records(&env);
        let mut values = record_map
            .get(name.clone())
            .unwrap_or_else(|| Map::new(&env));
        values.set(kind, value.clone());
        record_map.set(name.clone(), values);
        write_records(&env, &record_map);

        RecordSet { name, kind, value }.publish(&env);
    }

    /// The record `name` points at for `kind`.
    ///
    /// An expired name resolves to nothing even though the entry is still in
    /// storage: resolution is a property of the term, not of the key.
    pub fn resolve(env: Env, name: String, kind: RecordKind) -> Option<String> {
        let domain = domains(&env).get(name.clone())?;
        if domain.expires_at <= env.ledger().timestamp() {
            return None;
        }
        records(&env).get(name)?.get(kind)
    }
}

#[cfg(test)]
mod test;
