//! Detection of storage keys orphaned by an incomplete lifecycle.
//!
//! A contract that opens an order, escrow or position allocates keys for it and
//! is expected to release them when the entity closes, fills or is cancelled.
//! A missed `remove` does not fail the call that forgot it — it quietly keeps
//! paying rent long after the entity is gone. [`StorageLeakDetector`] makes that
//! visible by capturing the contract's storage before a lifecycle starts and
//! comparing it with the keys still live at teardown.
//!
//! ```text
//! let detector = StorageLeakDetector::start(&ctx.env, &contract_id);
//! // ... open escrow, fund, cancel ...
//! let leaks = detector.detect_orphaned_keys();
//! assert!(leaks.is_empty(), "Storage leak detected: {leaks:?}");
//! ```
//!
//! Only keys allocated *during* the lifecycle are considered, and only
//! persistent and temporary entries: instance storage is the contract's
//! singleton configuration and stays for its whole life. Keys an operation
//! rewrites without adding or removing are not leaks either — the detector
//! compares key identity, not values.
//!
//! Call [`StorageLeakDetector::checkpoint`] between phases to record where a key
//! was first seen. Without one, a leak is still detected but its allocation site
//! reads as unobserved.

use std::collections::BTreeSet;
use std::fmt;

use soroban_sdk::{Address, Env};

use crate::storage::{StorageEntry, StorageSnapshot, StorageTier};

/// One storage key that outlived the lifecycle that allocated it.
///
/// `byte_size` is the size of the key and value as the snapshot renders them,
/// which is an estimate of the ledger bytes the key occupies rather than a rent
/// quote. `allocation_site` is the label of the first checkpoint that saw the
/// key, or a note that no checkpoint recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageLeak {
    pub key: String,
    pub tier: StorageTier,
    pub byte_size: usize,
    pub allocation_site: String,
}

impl StorageLeak {
    /// `tier:key`, the identity the detector reports.
    pub fn qualified(&self) -> String {
        format!("{}:{}", self.tier.as_str(), self.key)
    }
}

impl fmt::Display for StorageLeak {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "storage leak: {} ({} bytes, allocated at {})",
            self.qualified(),
            self.byte_size,
            self.allocation_site
        )
    }
}

/// A captured `(tier, key)` set, used for both the baseline and each checkpoint.
type Identities = BTreeSet<(StorageTier, String)>;

/// One recorded phase of a lifecycle.
#[derive(Debug, Clone)]
struct Checkpoint {
    label: String,
    identities: Identities,
}

/// Watches one contract's storage across a lifecycle and reports keys that were
/// allocated but never released.
///
/// The detector is deliberately conservative. It flags only persistent and
/// temporary keys that are live at teardown and absent from the baseline, so a
/// test does not have to enumerate every permanent key a contract owns. Names
/// that are expected to survive — a singleton configuration record, a factory
/// registry — can be exempted with [`Self::ignoring_key`] and
/// [`Self::ignoring_pattern`].
///
/// The detector keeps a handle to the environment and contract it started on,
/// so every later call reads the same chain without the test repeating them.
/// The handle is a clone: it shares the host, so changes a contract call makes
/// are visible to the next capture.
pub struct StorageLeakDetector {
    env: Env,
    contract: Address,
    baseline: StorageSnapshot,
    ignored: BTreeSet<String>,
    ignored_patterns: Vec<String>,
    checkpoints: Vec<Checkpoint>,
}

impl StorageLeakDetector {
    /// Capture `contract`'s storage as the clean starting point.
    ///
    /// The address may be anything a contract call would take; an account
    /// address owns no contract data and therefore detects nothing.
    pub fn start(env: &Env, contract: &Address) -> Self {
        Self {
            env: env.clone(),
            contract: contract.clone(),
            baseline: StorageSnapshot::capture(env, contract),
            ignored: BTreeSet::new(),
            ignored_patterns: Vec::new(),
            checkpoints: Vec::new(),
        }
    }

    /// Exempt one exact key name from detection.
    ///
    /// The key is compared without its tier, so `"config"` exempts
    /// `persistent:config` and `temporary:config` alike. Chain the call:
    ///
    /// ```text
    /// let detector = StorageLeakDetector::start(&env, &contract).ignoring_key("config");
    /// ```
    pub fn ignoring_key(mut self, key: impl Into<String>) -> Self {
        self.ignored.insert(key.into());
        self
    }

    /// Exempt several exact key names.
    pub fn ignoring_keys<I, S>(mut self, keys: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.ignored.extend(keys.into_iter().map(Into::into));
        self
    }

    /// Exempt keys matching a wildcard pattern.
    ///
    /// The pattern matches either the bare key or `tier:key`. A trailing `*`
    /// matches a prefix (`"order:*"`), a leading `*` matches a suffix
    /// (`"*:cache"`), and `"*"` matches everything. This is how a test exempts
    /// a whole family of singleton records without naming each one.
    pub fn ignoring_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.ignored_patterns.push(pattern.into());
        self
    }

    /// Record the current storage under a phase label.
    ///
    /// The first checkpoint whose snapshot contains a leaked key is the
    /// allocation site attached to the leak. Call it after the step that
    /// allocates — `"open escrow"`, `"fund order"` — to turn a diagnostic into
    /// a pointer at the missed cleanup.
    pub fn checkpoint(&mut self, label: impl Into<String>) {
        let identities = identities(&StorageSnapshot::capture(&self.env, &self.contract));
        self.checkpoints.push(Checkpoint {
            label: label.into(),
            identities,
        });
    }

    /// The storage the detector started from.
    pub fn baseline(&self) -> &StorageSnapshot {
        &self.baseline
    }

    /// The phase labels recorded so far, in order.
    pub fn checkpoints(&self) -> Vec<&str> {
        self.checkpoints
            .iter()
            .map(|checkpoint| checkpoint.label.as_str())
            .collect()
    }

    /// Detect keys allocated during the lifecycle and still live at teardown.
    ///
    /// The result is sorted by tier and key, so a failure message is stable
    /// across runs. Instance storage is never reported, exact ignore lists are
    /// applied, and a key that already existed in the baseline is not a leak.
    pub fn detect_orphaned_keys(&self) -> Vec<StorageLeak> {
        self.detect_orphaned_keys_in(&StorageSnapshot::capture(&self.env, &self.contract))
    }

    /// Detect leaks against a snapshot the caller captured itself.
    ///
    /// Useful when a test already holds the teardown snapshot for a diff; the
    /// detector then adds only the leak analysis on top of it.
    pub fn detect_orphaned_keys_in(&self, current: &StorageSnapshot) -> Vec<StorageLeak> {
        let baseline = identities(&self.baseline);

        current
            .entries()
            .iter()
            .filter(|entry| self.is_leak(entry, &baseline))
            .map(|entry| StorageLeak {
                key: entry.key.clone(),
                tier: entry.tier,
                byte_size: entry.key.len() + entry.value.len(),
                allocation_site: self.allocation_site(entry),
            })
            .collect()
    }

    /// Fail unless no key was orphaned.
    ///
    /// The panic message lists every leak on its own line, so a failing test
    /// names the keys and the phases that allocated them rather than only the
    /// first one found.
    pub fn assert_no_orphaned_keys(&self) {
        let leaks = self.detect_orphaned_keys();
        assert!(
            leaks.is_empty(),
            "Storage leak detected: {}\n{}",
            leaks.len(),
            self.report(&leaks)
        );
    }

    /// Render leaks as one diagnostic line each.
    pub fn report(&self, leaks: &[StorageLeak]) -> String {
        if leaks.is_empty() {
            return "no orphaned storage keys".to_string();
        }
        leaks
            .iter()
            .map(|leak| format!("  {leak}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Whether one live entry should be reported as a leak.
    fn is_leak(&self, entry: &StorageEntry, baseline: &Identities) -> bool {
        // Instance storage holds the contract's singleton record for its whole
        // life; it is not an entity that can be closed.
        if entry.tier == StorageTier::Instance {
            return false;
        }
        if baseline.contains(&(entry.tier, entry.key.clone())) {
            return false;
        }
        !self.is_ignored(entry)
    }

    /// Whether an entry matches an exact ignore name or a wildcard pattern.
    fn is_ignored(&self, entry: &StorageEntry) -> bool {
        if self.ignored.contains(&entry.key) || self.ignored.contains(&entry.qualified()) {
            return true;
        }
        self.ignored_patterns
            .iter()
            .any(|pattern| matches_pattern(pattern, entry))
    }

    /// The label of the first checkpoint that saw the entry, if any.
    fn allocation_site(&self, entry: &StorageEntry) -> String {
        self.checkpoints
            .iter()
            .find(|checkpoint| {
                checkpoint
                    .identities
                    .contains(&(entry.tier, entry.key.clone()))
            })
            .map(|checkpoint| checkpoint.label.clone())
            .unwrap_or_else(|| "unobserved (no checkpoint recorded it)".to_string())
    }
}

/// The identities of a snapshot, for set comparisons that ignore values.
fn identities(snapshot: &StorageSnapshot) -> Identities {
    snapshot
        .entries()
        .iter()
        .map(|entry| (entry.tier, entry.key.clone()))
        .collect()
}

/// Match a wildcard pattern against a key or its `tier:key` form.
fn matches_pattern(pattern: &str, entry: &StorageEntry) -> bool {
    let qualified = entry.qualified();
    [entry.key.as_str(), qualified.as_str()]
        .iter()
        .any(|candidate| wildcard_matches(pattern, candidate))
}

/// One `*` wildcard, anchored at the start or the end.
fn wildcard_matches(pattern: &str, candidate: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match (pattern.strip_suffix('*'), pattern.strip_prefix('*')) {
        (Some(prefix), _) => candidate.starts_with(prefix),
        (_, Some(suffix)) => candidate.ends_with(suffix),
        _ => pattern == candidate,
    }
}

#[cfg(test)]
mod tests {
    use super::StorageLeakDetector;
    use crate::storage::StorageTier;
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env, Symbol,
    };

    #[contract]
    pub struct Vault;

    #[contractimpl]
    impl Vault {
        /// A permanent singleton, written to instance storage.
        pub fn initialize(env: Env, admin: Address) {
            env.storage()
                .instance()
                .set(&symbol_short!("admin"), &admin);
        }

        /// Open an escrow: one persistent key and one temporary quote.
        pub fn open(env: Env) {
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "escrow"), &100i128);
            env.storage()
                .temporary()
                .set(&Symbol::new(&env, "quote"), &7u32);
        }

        /// The complete close path: both of the keys `open` allocated go away.
        pub fn close(env: Env) {
            env.storage()
                .persistent()
                .remove(&Symbol::new(&env, "escrow"));
            env.storage()
                .temporary()
                .remove(&Symbol::new(&env, "quote"));
        }

        /// Cancels but forgets the temporary quote.
        pub fn cancel(env: Env) {
            env.storage()
                .persistent()
                .remove(&Symbol::new(&env, "escrow"));
        }

        /// Settles but forgets the persistent escrow record.
        pub fn settle(env: Env) {
            env.storage()
                .temporary()
                .remove(&Symbol::new(&env, "quote"));
        }

        /// A permanent configuration record that is expected to survive.
        pub fn write_config(env: Env) {
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "config"), &1u32);
        }

        /// A new instance key, which configuration may add at any time.
        pub fn write_flag(env: Env) {
            env.storage().instance().set(&symbol_short!("flag"), &true);
        }

        pub fn seed_ledger(env: Env) {
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "ledger"), &0u32);
        }
    }

    struct Fixture {
        env: Env,
        contract: Address,
    }

    fn setup() -> Fixture {
        let env = Env::default();
        env.mock_all_auths();
        let contract = env.register(Vault, ());
        VaultClient::new(&env, &contract).initialize(&Address::generate(&env));
        Fixture { env, contract }
    }

    impl Fixture {
        fn client(&self) -> VaultClient<'_> {
            VaultClient::new(&self.env, &self.contract)
        }

        fn detector(&self) -> StorageLeakDetector {
            StorageLeakDetector::start(&self.env, &self.contract)
        }
    }

    #[test]
    fn a_clean_close_reports_no_leaks() {
        let fixture = setup();
        let detector = fixture.detector();

        fixture.client().open();
        fixture.client().close();

        assert!(detector.detect_orphaned_keys().is_empty());
    }

    #[test]
    fn a_forgotten_temporary_key_is_detected() {
        let fixture = setup();
        let detector = fixture.detector();

        fixture.client().open();
        fixture.client().cancel();

        let leaks = detector.detect_orphaned_keys();
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].key, "quote");
        assert_eq!(leaks[0].tier, StorageTier::Temporary);
        assert_eq!(leaks[0].qualified(), "temporary:quote");
        assert!(leaks[0].byte_size > 0);
    }

    #[test]
    fn a_forgotten_persistent_key_is_detected() {
        let fixture = setup();
        let detector = fixture.detector();

        fixture.client().open();
        fixture.client().settle();

        let leaks = detector.detect_orphaned_keys();
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].key, "escrow");
        assert_eq!(leaks[0].tier, StorageTier::Persistent);
        assert!(leaks[0].byte_size >= "escrow".len());
    }

    #[test]
    fn the_leak_reports_the_phase_that_allocated_it() {
        let fixture = setup();
        let client = fixture.client();
        let mut detector = fixture.detector();

        client.open();
        detector.checkpoint("open escrow");
        client.cancel();
        detector.checkpoint("cancel escrow");

        let leaks = detector.detect_orphaned_keys();
        assert_eq!(leaks.len(), 1);
        assert_eq!(leaks[0].allocation_site, "open escrow");
        assert_eq!(detector.checkpoints(), vec!["open escrow", "cancel escrow"]);
    }

    #[test]
    fn a_leak_without_a_checkpoint_says_so() {
        let fixture = setup();
        let detector = fixture.detector();

        fixture.client().open();
        fixture.client().cancel();

        let leaks = detector.detect_orphaned_keys();
        assert_eq!(leaks.len(), 1);
        assert!(leaks[0].allocation_site.contains("unobserved"));
    }

    #[test]
    fn an_exact_ignore_exempts_a_permanent_record() {
        let fixture = setup();
        let detector = fixture.detector().ignoring_key("config");

        fixture.client().write_config();

        assert!(detector.detect_orphaned_keys().is_empty());
    }

    #[test]
    fn ignore_patterns_exempt_a_family_of_keys() {
        let fixture = setup();
        let detector = fixture
            .detector()
            .ignoring_pattern("esc*")
            .ignoring_pattern("*:quote");

        fixture.client().open();
        fixture.client().cancel();
        fixture.client().settle();

        assert!(detector.detect_orphaned_keys().is_empty());
    }

    #[test]
    fn a_qualified_ignore_only_exempts_its_own_tier() {
        let fixture = setup();
        let exempt = fixture.detector().ignoring_key("temporary:quote");
        let wrong_tier = fixture.detector().ignoring_key("persistent:quote");

        fixture.client().open();
        fixture.client().cancel();

        assert!(exempt.detect_orphaned_keys().is_empty());

        let leaks = wrong_tier.detect_orphaned_keys();
        assert_eq!(leaks.len(), 1, "the temporary quote is still a leak");
    }

    #[test]
    fn instance_storage_is_never_reported() {
        let fixture = setup();
        let detector = fixture.detector();

        // `initialize` wrote instance storage before the detector started; even
        // a later instance key is configuration, not a closable entity.
        fixture.client().write_flag();
        fixture.client().open();
        fixture.client().close();

        assert!(detector.detect_orphaned_keys().is_empty());
    }

    #[test]
    fn keys_present_in_the_baseline_are_not_leaks() {
        let fixture = setup();
        fixture.client().seed_ledger();

        let detector = fixture.detector();
        // Open and close allocate and free only the lifecycle keys; the
        // pre-existing ledger entry survives and is still not a leak.
        fixture.client().open();
        fixture.client().close();

        assert!(detector.detect_orphaned_keys().is_empty());
    }

    #[test]
    fn a_growing_ignore_list_keeps_earlier_entries() {
        let fixture = setup();
        let detector = fixture
            .detector()
            .ignoring_keys(["config", "ledger"])
            .ignoring_key("temporary:quote");

        fixture.client().open();
        fixture.client().cancel();
        fixture.client().write_config();

        assert!(detector.detect_orphaned_keys().is_empty());
    }

    #[test]
    fn a_leak_renders_as_a_readable_diagnostic() {
        let fixture = setup();
        let detector = fixture.detector();
        let mut detector = detector;

        fixture.client().open();
        detector.checkpoint("open escrow");
        fixture.client().cancel();
        let leaks = detector.detect_orphaned_keys();

        let line = leaks[0].to_string();
        assert!(line.contains("storage leak: temporary:quote"));
        assert!(line.contains("bytes"));
        assert!(line.contains("allocated at open escrow"));
        assert!(detector.report(&leaks).contains("temporary:quote"));
        assert_eq!(detector.report(&[]), "no orphaned storage keys");
    }

    #[test]
    #[should_panic(expected = "Storage leak detected: 1")]
    fn the_assertion_names_the_leak_count_and_the_key() {
        let fixture = setup();
        let detector = fixture.detector();

        fixture.client().open();
        fixture.client().cancel();

        detector.assert_no_orphaned_keys();
    }

    #[test]
    fn detecting_against_a_teardown_snapshot_matches_a_fresh_capture() {
        let fixture = setup();
        let detector = fixture.detector();
        fixture.client().open();
        fixture.client().cancel();

        let captured = crate::storage::StorageSnapshot::capture(&fixture.env, &fixture.contract);
        assert_eq!(
            detector.detect_orphaned_keys_in(&captured),
            detector.detect_orphaned_keys()
        );
    }
}
