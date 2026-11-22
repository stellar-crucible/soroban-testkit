//! Rent-aware storage tier recommendations.
//!
//! A contract pays rent for every byte it keeps alive, and the tier an entry
//! lives in decides both how long that is and what a call pays to reach it.
//! Temporary entries are billed for a shorter lifetime than persistent ones;
//! instance keys ride the single contract-instance entry, so reading them costs
//! no separate ledger-entry lookup and they carry no per-entry overhead.
//!
//! [`StorageOptimizationRecommender`] records how often each key was read and
//! written over a test execution sequence, classifies the key from that pattern,
//! and reports the migrations that would lower its rent:
//!
//! ```text
//! use soroban_testkit_core::optimizer::StorageOptimizationRecommender;
//! use soroban_testkit_core::storage::{StorageSnapshot, StorageTier};
//!
//! let mut recommender = StorageOptimizationRecommender::new();
//! let before = StorageSnapshot::capture(&env, &contract);
//! client.transfer(&from, &to, &100);
//! let after = StorageSnapshot::capture(&env, &contract);
//!
//! recommender.record_writes_from(&before.diff(&after));
//! recommender.record_read("balances", StorageTier::Persistent, 96);
//! recommender.record_read("metadata", StorageTier::Persistent, 12);
//!
//! println!("{}", recommender.to_markdown());
//! ```
//!
//! Reads are recorded by the test because the SDK does not report which keys a
//! call touched — [`StorageSnapshot`] shows what storage *looks like*, not what
//! was read out of it. [`StorageOptimizationRecommender::record_reads_from`]
//! covers the common case where every key a snapshot holds is a key the step
//! depended on; [`StorageOptimizationRecommender::record_read`] is the precise
//! form.
//!
//! # What the numbers are
//!
//! Rent is `size in bytes × ledgers kept alive × the network's per-byte,
//! per-ledger rate`. The rate and the maximum lifetimes are network parameters
//! that change with protocol upgrades, so [`RentModel`] carries them explicitly
//! with documented defaults rather than pretending they are constants. The
//! figures a report prints are a comparison between two tiers on the same model
//! — a saving to confirm against the current network settings, not a fee quote.

use std::collections::BTreeMap;
use std::fmt;

use crate::storage::{StorageDiff, StorageEntry, StorageSnapshot, StorageTier};

/// How often a key was touched, and what tier it is stored in now.
///
/// One record per key, accumulated over the whole sequence the recommender
/// observed. `first_seen` and `last_seen` are positions in that sequence, so a
/// key written once at the start and never again reads back as such.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyAccess {
    pub key: String,
    /// The tier the key was last observed in.
    pub tier: StorageTier,
    pub reads: u32,
    pub writes: u32,
    /// Approximate entry size in bytes: the key plus the rendered value.
    pub size_bytes: u64,
    /// Position of the first recorded access, counting every recorded read and write.
    pub first_seen: u64,
    /// Position of the most recent recorded access.
    pub last_seen: u64,
}

impl KeyAccess {
    pub fn accesses(&self) -> u32 {
        self.reads.saturating_add(self.writes)
    }

    /// `tier:key`, the form a report and its failure messages quote.
    pub fn qualified(&self) -> String {
        format!("{}:{}", self.tier.as_str(), self.key)
    }
}

/// What a key's access pattern says about the tier it belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyClass {
    /// Read or written often enough that the cost of reaching it matters.
    Hot,
    /// Touched rarely; the tier it lives in hardly changes its rent.
    Cold,
    /// Written once and consumed at most once inside the sequence.
    Ephemeral,
}

impl KeyClass {
    /// Stable label, used in reports and documentation.
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyClass::Hot => "hot",
            KeyClass::Cold => "cold",
            KeyClass::Ephemeral => "ephemeral",
        }
    }
}

/// Where the boundary between hot, cold and ephemeral sits.
///
/// The defaults describe a test sequence long enough for a frequently used key
/// to be touched several times: four accesses in total, or three reads, is hot;
/// a key written once and read at most once is ephemeral; everything else is
/// cold. Override them when the sequence a test records is much shorter or much
/// longer than that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessThresholds {
    /// Total reads plus writes that make a key hot.
    pub hot_accesses: u32,
    /// Reads that make a key hot regardless of how often it was written.
    pub hot_reads: u32,
    /// Reads an ephemeral key may have: written once and consumed, then gone.
    pub ephemeral_max_reads: u32,
    /// Largest key, in bytes, that is a candidate for instance storage.
    ///
    /// Instance storage is one entry shared by the whole contract and grows with
    /// every key added to it, so only small singletons are recommended for it.
    pub singleton_max_bytes: u64,
}

impl Default for AccessThresholds {
    fn default() -> Self {
        Self {
            hot_accesses: 4,
            hot_reads: 3,
            ephemeral_max_reads: 1,
            singleton_max_bytes: 128,
        }
    }
}

impl AccessThresholds {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn hot_accesses(mut self, accesses: u32) -> Self {
        self.hot_accesses = accesses;
        self
    }

    pub fn hot_reads(mut self, reads: u32) -> Self {
        self.hot_reads = reads;
        self
    }

    pub fn ephemeral_max_reads(mut self, reads: u32) -> Self {
        self.ephemeral_max_reads = reads;
        self
    }

    pub fn singleton_max_bytes(mut self, bytes: u64) -> Self {
        self.singleton_max_bytes = bytes;
        self
    }
}

/// The rent parameters a report's estimates are computed with.
///
/// Rent on Soroban is charged per byte per ledger the entry stays live, so the
/// two things that move a bill are the entry's size and how long it is kept.
/// The defaults are a stable, documented approximation: one byte kept for one
/// ledger costs 0.016 stroop (16,000 micro-stroops), persistent and instance
/// entries are modelled over 3,110,400 ledgers, temporary entries over
/// 1,555,200, and each persistent or temporary entry carries 48 bytes of ledger
/// metadata. Instance keys carry no per-entry overhead because they live inside
/// the contract-instance entry that the contract already pays for.
///
/// Every field is public, so a test can put the current network values in
/// instead of the defaults; the recommendation is unchanged by the rate, since
/// it compares two tiers on the same model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RentModel {
    /// Stroops per byte per ledger, in millionths of a stroop (16,000 = 0.016).
    pub stroops_per_byte_ledger_micros: u64,
    /// Ledgers a persistent entry is modelled as kept alive for.
    pub persistent_ttl_ledgers: u32,
    /// Ledgers a temporary entry is modelled as kept alive for.
    pub temporary_ttl_ledgers: u32,
    /// Ledgers an instance entry is modelled as kept alive for.
    pub instance_ttl_ledgers: u32,
    /// Ledger-entry metadata counted against each persistent or temporary key.
    pub entry_overhead_bytes: u64,
}

impl Default for RentModel {
    fn default() -> Self {
        Self {
            stroops_per_byte_ledger_micros: 16_000,
            persistent_ttl_ledgers: 3_110_400,
            temporary_ttl_ledgers: 1_555_200,
            instance_ttl_ledgers: 3_110_400,
            entry_overhead_bytes: 48,
        }
    }
}

impl RentModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long this model keeps entries of `tier` alive.
    pub fn ttl_ledgers(&self, tier: StorageTier) -> u32 {
        match tier {
            StorageTier::Instance => self.instance_ttl_ledgers,
            StorageTier::Persistent => self.persistent_ttl_ledgers,
            StorageTier::Temporary => self.temporary_ttl_ledgers,
        }
    }

    /// Bytes of ledger metadata this model charges an entry of `tier`.
    pub fn overhead_bytes(&self, tier: StorageTier) -> u64 {
        match tier {
            // Instance keys are part of the contract-instance entry, not entries
            // of their own.
            StorageTier::Instance => 0,
            StorageTier::Persistent | StorageTier::Temporary => self.entry_overhead_bytes,
        }
    }

    /// Estimated rent, in stroops, of keeping `size_bytes` in `tier`.
    pub fn rent_stroops(&self, tier: StorageTier, size_bytes: u64) -> u128 {
        let bytes = u128::from(size_bytes) + u128::from(self.overhead_bytes(tier));
        let ledgers = u128::from(self.ttl_ledgers(tier));
        bytes * ledgers * u128::from(self.stroops_per_byte_ledger_micros) / 1_000_000
    }

    pub fn with_stroops_per_byte_ledger_micros(mut self, micros: u64) -> Self {
        self.stroops_per_byte_ledger_micros = micros;
        self
    }

    pub fn with_entry_overhead_bytes(mut self, bytes: u64) -> Self {
        self.entry_overhead_bytes = bytes;
        self
    }

    /// Sets how long entries of `tier` are kept alive in the model.
    pub fn with_ttl_ledgers(mut self, tier: StorageTier, ledgers: u32) -> Self {
        match tier {
            StorageTier::Instance => self.instance_ttl_ledgers = ledgers,
            StorageTier::Persistent => self.persistent_ttl_ledgers = ledgers,
            StorageTier::Temporary => self.temporary_ttl_ledgers = ledgers,
        }
        self
    }
}

/// One suggested tier migration and what it is expected to save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TierRecommendation {
    pub key: String,
    pub class: KeyClass,
    pub current: StorageTier,
    pub recommended: StorageTier,
    pub reads: u32,
    pub writes: u32,
    pub size_bytes: u64,
    /// Estimated rent of the key in its current tier.
    pub current_rent_stroops: u128,
    /// Estimated rent of the key once moved to the recommended tier.
    pub projected_rent_stroops: u128,
    /// Why the pattern points at that tier.
    pub rationale: String,
}

impl TierRecommendation {
    /// Stroops the migration is expected to save.
    pub fn saving_stroops(&self) -> u128 {
        self.current_rent_stroops
            .saturating_sub(self.projected_rent_stroops)
    }

    /// The saving as a percentage of the key's current rent.
    pub fn saving_percent(&self) -> f64 {
        if self.current_rent_stroops == 0 {
            0.0
        } else {
            self.saving_stroops() as f64 * 100.0 / self.current_rent_stroops as f64
        }
    }
}

/// The access patterns a sequence showed and the migrations they imply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizationReport {
    accesses: Vec<KeyAccess>,
    recommendations: Vec<TierRecommendation>,
    thresholds: AccessThresholds,
    model: RentModel,
    operations: u64,
}

impl OptimizationReport {
    /// Every key the sequence touched, in key order.
    pub fn accesses(&self) -> &[KeyAccess] {
        &self.accesses
    }

    /// The migrations the report suggests, largest saving first.
    pub fn recommendations(&self) -> &[TierRecommendation] {
        &self.recommendations
    }

    pub fn recommendation_for(&self, key: &str) -> Option<&TierRecommendation> {
        self.recommendations
            .iter()
            .find(|recommendation| recommendation.key == key)
    }

    /// The class the access pattern assigns a key.
    pub fn class_of(&self, key: &str) -> Option<KeyClass> {
        self.accesses
            .iter()
            .find(|access| access.key == key)
            .map(|access| classify(access, &self.thresholds))
    }

    pub fn thresholds(&self) -> AccessThresholds {
        self.thresholds
    }

    pub fn model(&self) -> RentModel {
        self.model
    }

    /// How many reads and writes were recorded in total.
    pub fn operations(&self) -> u64 {
        self.operations
    }

    /// Estimated rent of every tracked key in the tier it is in now.
    pub fn current_rent_stroops(&self) -> u128 {
        self.accesses
            .iter()
            .map(|access| self.model.rent_stroops(access.tier, access.size_bytes))
            .sum()
    }

    /// Estimated rent after every recommendation is adopted.
    pub fn projected_rent_stroops(&self) -> u128 {
        self.current_rent_stroops()
            .saturating_sub(self.projected_saving_stroops())
    }

    /// Stroops the recommendations are expected to save together.
    pub fn projected_saving_stroops(&self) -> u128 {
        self.recommendations
            .iter()
            .map(TierRecommendation::saving_stroops)
            .sum()
    }

    /// The total saving as a percentage of the current rent.
    pub fn projected_saving_percent(&self) -> f64 {
        let current = self.current_rent_stroops();
        if current == 0 {
            0.0
        } else {
            self.projected_saving_stroops() as f64 * 100.0 / current as f64
        }
    }

    /// The report as Markdown: the access pattern, the suggested migrations,
    /// and the model the figures were computed with.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("# Storage optimization report\n\n");
        out.push_str(&format!(
            "Analyzed **{}** storage key(s) over **{}** recorded operation(s), with tiers compared on the same rent model.\n\n",
            self.accesses.len(),
            self.operations,
        ));

        out.push_str("## Access patterns\n\n");
        if self.accesses.is_empty() {
            out.push_str("No storage key was recorded.\n\n");
        } else {
            out.push_str("| Key | Tier | Reads | Writes | Size (bytes) | Class |\n");
            out.push_str("|-----|------|-------|--------|--------------|-------|\n");
            for access in &self.accesses {
                let class = classify(access, &self.thresholds);
                out.push_str(&format!(
                    "| `{}` | {} | {} | {} | {} | {} |\n",
                    access.key,
                    access.tier.as_str(),
                    access.reads,
                    access.writes,
                    access.size_bytes,
                    class.as_str(),
                ));
            }
            out.push('\n');
        }

        out.push_str("## Recommended tier migrations\n\n");
        if self.recommendations.is_empty() {
            out.push_str(
                "No migration is recommended: every key's access pattern matches the tier it is stored in.\n\n",
            );
        } else {
            out.push_str("| Key | From | To | Projected saving (stroops) | Saving |\n");
            out.push_str("|-----|------|----|----------------------------|--------|\n");
            for recommendation in &self.recommendations {
                out.push_str(&format!(
                    "| `{}` | {} | {} | {} | {:.1}% |\n",
                    recommendation.key,
                    recommendation.current.as_str(),
                    recommendation.recommended.as_str(),
                    recommendation.saving_stroops(),
                    recommendation.saving_percent(),
                ));
            }
            out.push('\n');

            for recommendation in &self.recommendations {
                out.push_str(&format!(
                    "### `{}`: {} → {}\n\n{}\n\n",
                    recommendation.key,
                    recommendation.current.as_str(),
                    recommendation.recommended.as_str(),
                    recommendation.rationale,
                ));
                out.push_str(&format!(
                    "Estimated rent {} → {} stroops ({} saved).\n\n",
                    recommendation.current_rent_stroops,
                    recommendation.projected_rent_stroops,
                    recommendation.saving_stroops(),
                ));
            }
        }

        out.push_str(&format!(
            "**Projected rent: {} → {} stroops ({} saved, {:.1}%).**\n\n",
            self.current_rent_stroops(),
            self.projected_rent_stroops(),
            self.projected_saving_stroops(),
            self.projected_saving_percent(),
        ));

        out.push_str("---\n\n");
        out.push_str(&format!(
            "Rent model: {} micro-stroops per byte per ledger, TTLs of {} ledgers for persistent, {} for temporary and {} for instance entries, plus {} bytes of per-entry overhead. Rent rates are network parameters, so read these figures as comparable savings and override `RentModel` with the current network values before quoting them as fees.\n",
            self.model.stroops_per_byte_ledger_micros,
            self.model.persistent_ttl_ledgers,
            self.model.temporary_ttl_ledgers,
            self.model.instance_ttl_ledgers,
            self.model.entry_overhead_bytes,
        ));

        out
    }
}

impl fmt::Display for OptimizationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_markdown())
    }
}

/// Tracks storage access across a test execution sequence and recommends tiers.
///
/// Reads and writes are recorded explicitly because the SDK reports the state a
/// call left behind, not the keys it read. A snapshot diff supplies the writes;
/// [`StorageOptimizationRecommender::record_reads_from`] supplies reads for the
/// keys a snapshot holds, and [`StorageOptimizationRecommender::record_read`]
/// names one key when a test knows exactly which ones a call depended on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOptimizationRecommender {
    accesses: BTreeMap<String, KeyAccess>,
    thresholds: AccessThresholds,
    model: RentModel,
    next_tick: u64,
}

impl Default for StorageOptimizationRecommender {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageOptimizationRecommender {
    pub fn new() -> Self {
        Self {
            accesses: BTreeMap::new(),
            thresholds: AccessThresholds::default(),
            model: RentModel::default(),
            next_tick: 0,
        }
    }

    pub fn with_thresholds(mut self, thresholds: AccessThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }

    pub fn with_model(mut self, model: RentModel) -> Self {
        self.model = model;
        self
    }

    pub fn thresholds(&self) -> AccessThresholds {
        self.thresholds
    }

    pub fn model(&self) -> RentModel {
        self.model
    }

    /// Every key tracked so far, in key order.
    pub fn keys(&self) -> Vec<&KeyAccess> {
        self.accesses.values().collect()
    }

    /// Records one read of `key`, which lives in `tier` at `size_bytes`.
    pub fn record_read(
        &mut self,
        key: impl Into<String>,
        tier: StorageTier,
        size_bytes: u64,
    ) -> &mut Self {
        self.touch(key.into(), tier, size_bytes).reads += 1;
        self
    }

    /// Records one write of `key`, which lives in `tier` at `size_bytes`.
    pub fn record_write(
        &mut self,
        key: impl Into<String>,
        tier: StorageTier,
        size_bytes: u64,
    ) -> &mut Self {
        self.touch(key.into(), tier, size_bytes).writes += 1;
        self
    }

    /// Records a read of every key a snapshot holds.
    ///
    /// This is the coarse form: it says the step that produced the snapshot
    /// depended on the whole live set. Use [`Self::record_read`] when a test
    /// knows which keys the call actually looked at, so a key that merely
    /// exists is not counted as hot.
    pub fn record_reads_from(&mut self, snapshot: &StorageSnapshot) -> &mut Self {
        for entry in snapshot.entries() {
            self.record_read(entry.key.clone(), entry.tier, entry_size(entry));
        }
        self
    }

    /// Records a write for every key a diff reports as added or rewritten.
    pub fn record_writes_from(&mut self, diff: &StorageDiff) -> &mut Self {
        for entry in diff.added() {
            self.record_write(entry.key.clone(), entry.tier, entry_size(entry));
        }
        for change in diff.modified() {
            self.record_write(
                change.key.clone(),
                change.tier,
                size_bytes(&change.key, &change.after),
            );
        }
        self
    }

    /// The report this sequence produces.
    pub fn report(&self) -> OptimizationReport {
        OptimizationReport {
            accesses: self.accesses.values().cloned().collect(),
            recommendations: self.recommendations(),
            thresholds: self.thresholds,
            model: self.model,
            operations: self.next_tick,
        }
    }

    /// The report as Markdown, for a test that prints what it recommends.
    pub fn to_markdown(&self) -> String {
        self.report().to_markdown()
    }

    fn touch(&mut self, key: String, tier: StorageTier, size_bytes: u64) -> &mut KeyAccess {
        let tick = self.next_tick;
        self.next_tick += 1;

        let access = self
            .accesses
            .entry(key.clone())
            .or_insert_with(|| KeyAccess {
                key,
                tier,
                reads: 0,
                writes: 0,
                size_bytes,
                first_seen: tick,
                last_seen: tick,
            });
        access.tier = tier;
        access.size_bytes = size_bytes;
        access.last_seen = tick;
        access
    }

    fn recommendations(&self) -> Vec<TierRecommendation> {
        let mut recommendations = Vec::new();

        for access in self.accesses.values() {
            let class = classify(access, &self.thresholds);
            let Some((recommended, rationale)) = self.suggest(access, class) else {
                continue;
            };
            recommendations.push(TierRecommendation {
                key: access.key.clone(),
                class,
                current: access.tier,
                recommended,
                reads: access.reads,
                writes: access.writes,
                size_bytes: access.size_bytes,
                current_rent_stroops: self.model.rent_stroops(access.tier, access.size_bytes),
                projected_rent_stroops: self.model.rent_stroops(recommended, access.size_bytes),
                rationale,
            });
        }

        recommendations.sort_by(|left, right| {
            right
                .saving_stroops()
                .cmp(&left.saving_stroops())
                .then_with(|| left.key.cmp(&right.key))
        });
        recommendations
    }

    /// The tier a key's pattern points at, unless it is already there.
    fn suggest(&self, access: &KeyAccess, class: KeyClass) -> Option<(StorageTier, String)> {
        match class {
            KeyClass::Ephemeral if access.tier != StorageTier::Temporary => Some((
                StorageTier::Temporary,
                format!(
                    "Written {} time(s) and read {} time(s): the entry is consumed inside the sequence, so temporary storage keeps it for {} ledgers instead of {} and bills roughly half the rent.",
                    access.writes,
                    access.reads,
                    self.model.ttl_ledgers(StorageTier::Temporary),
                    self.model.ttl_ledgers(StorageTier::Persistent),
                ),
            )),
            KeyClass::Hot
                if access.tier != StorageTier::Instance
                    && access.reads > access.writes
                    && access.size_bytes <= self.thresholds.singleton_max_bytes =>
            {
                Some((
                    StorageTier::Instance,
                    format!(
                        "Read {} time(s) against {} write(s) at {} bytes: instance keys load with the contract instance, so a call stops paying for a separate ledger entry and the key carries no per-entry rent overhead.",
                        access.reads, access.writes, access.size_bytes,
                    ),
                ))
            }
            _ => None,
        }
    }
}

/// Reads a key's class out of its pattern.
fn classify(access: &KeyAccess, thresholds: &AccessThresholds) -> KeyClass {
    if access.reads >= thresholds.hot_reads || access.accesses() >= thresholds.hot_accesses {
        KeyClass::Hot
    } else if access.writes == 1 && access.reads <= thresholds.ephemeral_max_reads {
        KeyClass::Ephemeral
    } else {
        KeyClass::Cold
    }
}

/// The size a recommender charges a live entry: its key plus its rendered value.
fn entry_size(entry: &StorageEntry) -> u64 {
    size_bytes(&entry.key, &entry.value)
}

fn size_bytes(key: &str, value: &str) -> u64 {
    (key.len() + value.len()) as u64
}

#[cfg(test)]
mod tests {
    use super::{
        AccessThresholds, KeyClass, RentModel, StorageOptimizationRecommender, TierRecommendation,
    };
    use crate::storage::{StorageSnapshot, StorageTier};
    use soroban_sdk::{
        contract, contractimpl, symbol_short, testutils::Address as _, Address, Env, Map,
    };

    // A token's shape: a singleton metadata value, a hot balances map, and the
    // admin address that belongs with the contract instance.
    #[contract]
    pub struct TokenContract;

    #[contractimpl]
    impl TokenContract {
        pub fn init(env: Env, admin: Address) {
            env.storage()
                .instance()
                .set(&symbol_short!("admin"), &admin);
            env.storage()
                .persistent()
                .set(&symbol_short!("metadata"), &1_000u32);
            env.storage()
                .persistent()
                .set(&symbol_short!("balances"), &Map::<Address, i128>::new(&env));
        }

        pub fn mint(env: Env, to: Address, amount: i128) {
            let key = symbol_short!("balances");
            let mut balances: Map<Address, i128> = env.storage().persistent().get(&key).unwrap();
            let balance = balances.get(to.clone()).unwrap_or(0);
            balances.set(to, balance + amount);
            env.storage().persistent().set(&key, &balances);
        }

        pub fn metadata(env: Env) -> u32 {
            env.storage()
                .persistent()
                .get(&symbol_short!("metadata"))
                .unwrap()
        }
    }

    // An escrow's shape: a config singleton read on every call, and a pending
    // payout entry that is written once and consumed once.
    #[contract]
    pub struct EscrowContract;

    #[contractimpl]
    impl EscrowContract {
        pub fn open(env: Env) {
            env.storage()
                .persistent()
                .set(&symbol_short!("config"), &7u32);
            env.storage()
                .persistent()
                .set(&symbol_short!("pending"), &9u32);
        }
    }

    fn token() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(TokenContract, ());
        let admin = Address::generate(&env);
        TokenContractClient::new(&env, &contract).init(&admin);
        (env, contract)
    }

    fn escrow() -> (Env, Address) {
        let env = Env::default();
        let contract = env.register(EscrowContract, ());
        EscrowContractClient::new(&env, &contract).open();
        (env, contract)
    }

    #[test]
    fn token_sequence_recommends_instance_storage_for_read_heavy_metadata() {
        let (env, contract) = token();
        let client = TokenContractClient::new(&env, &contract);
        let holder = Address::generate(&env);
        let mut recommender = StorageOptimizationRecommender::new();

        for amount in [100i128, 50, 25] {
            let before = StorageSnapshot::capture(&env, &contract);
            client.mint(&holder, &amount);
            let after = StorageSnapshot::capture(&env, &contract);

            recommender.record_writes_from(&before.diff(&after));
            recommender.record_reads_from(&after);
            let metadata = after.get("metadata").expect("metadata lives in storage");
            recommender.record_read("metadata", metadata.tier, metadata.value.len() as u64 + 8);
        }

        let report = recommender.report();
        let recommendation = report
            .recommendation_for("metadata")
            .expect("metadata is read on every call and small");

        assert_eq!(report.class_of("metadata"), Some(KeyClass::Hot));
        assert_eq!(recommendation.class, KeyClass::Hot);
        assert_eq!(recommendation.current, StorageTier::Persistent);
        assert_eq!(recommendation.recommended, StorageTier::Instance);
        assert!(recommendation.saving_stroops() > 0);
        assert!(recommendation.rationale.contains("instance"));
    }

    #[test]
    fn token_balances_is_hot_but_stays_persistent() {
        let (env, contract) = token();
        let client = TokenContractClient::new(&env, &contract);
        let holder = Address::generate(&env);
        let mut recommender = StorageOptimizationRecommender::new();

        for amount in [100i128, 50, 25] {
            let before = StorageSnapshot::capture(&env, &contract);
            client.mint(&holder, &amount);
            let after = StorageSnapshot::capture(&env, &contract);

            recommender.record_writes_from(&before.diff(&after));
            recommender.record_reads_from(&after);
        }

        let report = recommender.report();
        assert_eq!(report.class_of("balances"), Some(KeyClass::Hot));
        assert!(
            report.recommendation_for("balances").is_none(),
            "a key written as often as it is read is not a singleton"
        );
    }

    #[test]
    fn escrow_sequence_recommends_temporary_and_instance_storage() {
        let (_env, _contract) = escrow();
        let mut recommender = StorageOptimizationRecommender::new();

        for _ in 0..4 {
            recommender.record_read("config", StorageTier::Persistent, 12);
        }
        recommender.record_write("config", StorageTier::Persistent, 12);
        recommender.record_write("pending", StorageTier::Persistent, 20);
        recommender.record_read("pending", StorageTier::Persistent, 20);

        let report = recommender.report();
        assert_eq!(report.accesses().len(), 2);
        assert_eq!(
            report.class_of("pending").expect("pending was tracked"),
            KeyClass::Ephemeral
        );
        assert_eq!(
            report
                .recommendation_for("pending")
                .expect("a write-once entry belongs in temporary storage")
                .recommended,
            StorageTier::Temporary
        );
        assert_eq!(
            report
                .recommendation_for("config")
                .expect("a hot singleton belongs in instance storage")
                .recommended,
            StorageTier::Instance
        );

        let markdown = report.to_markdown();
        assert!(markdown.starts_with("# Storage optimization report"));
        assert!(markdown.contains("## Access patterns"));
        assert!(markdown.contains("| `pending` | persistent | 1 | 1 |"));
        assert!(markdown.contains("## Recommended tier migrations"));
        assert!(markdown.contains("### `config`: persistent → instance"));
        assert!(markdown.contains("### `pending`: persistent → temporary"));
    }

    #[test]
    fn recommendations_are_ordered_by_saving() {
        let mut recommender = StorageOptimizationRecommender::new();
        recommender.record_write("small", StorageTier::Persistent, 8);
        recommender.record_write("large", StorageTier::Persistent, 512);

        let report = recommender.report();
        let savings: Vec<u128> = report
            .recommendations()
            .iter()
            .map(TierRecommendation::saving_stroops)
            .collect();

        assert_eq!(report.recommendations().len(), 2);
        assert_eq!(savings[0], report.recommendations()[0].saving_stroops());
        assert!(savings[0] > savings[1]);
        assert_eq!(report.recommendations()[0].key, "large");
    }

    #[test]
    fn classification_follows_the_thresholds() {
        let mut recommender = StorageOptimizationRecommender::new();

        recommender.record_read("frequent", StorageTier::Persistent, 10);
        recommender.record_read("frequent", StorageTier::Persistent, 10);
        recommender.record_read("frequent", StorageTier::Persistent, 10);
        recommender.record_write("once", StorageTier::Persistent, 10);
        recommender.record_read("once", StorageTier::Persistent, 10);
        recommender.record_write("busy", StorageTier::Persistent, 10);
        recommender.record_write("busy", StorageTier::Persistent, 10);

        let report = recommender.report();
        assert_eq!(report.class_of("frequent"), Some(KeyClass::Hot));
        assert_eq!(report.class_of("once"), Some(KeyClass::Ephemeral));
        assert_eq!(report.class_of("busy"), Some(KeyClass::Cold));
        assert!(report.recommendation_for("busy").is_none());
        assert_eq!(report.class_of("absent"), None);
    }

    #[test]
    fn a_short_sequence_can_lower_the_hot_thresholds() {
        let mut recommender = StorageOptimizationRecommender::new()
            .with_thresholds(AccessThresholds::new().hot_reads(2).hot_accesses(2));

        recommender.record_read("config", StorageTier::Persistent, 12);
        recommender.record_read("config", StorageTier::Persistent, 12);

        let report = recommender.report();
        assert_eq!(report.class_of("config"), Some(KeyClass::Hot));
        assert_eq!(
            report
                .recommendation_for("config")
                .map(|recommendation| recommendation.recommended),
            Some(StorageTier::Instance)
        );
    }

    #[test]
    fn no_recommendation_when_the_tier_already_matches_the_pattern() {
        let mut recommender = StorageOptimizationRecommender::new();

        for _ in 0..3 {
            recommender.record_read("admin", StorageTier::Instance, 40);
        }
        recommender.record_write("nonce", StorageTier::Temporary, 12);
        recommender.record_read("nonce", StorageTier::Temporary, 12);

        let report = recommender.report();
        assert!(report.recommendations().is_empty());
        assert_eq!(report.projected_saving_stroops(), 0);
        assert_eq!(report.projected_saving_percent(), 0.0);
        assert!(report.to_markdown().contains("No migration is recommended"));
    }

    #[test]
    fn an_empty_sequence_produces_an_empty_report() {
        let report = StorageOptimizationRecommender::new().report();

        assert!(report.accesses().is_empty());
        assert!(report.recommendations().is_empty());
        assert_eq!(report.operations(), 0);
        assert_eq!(report.current_rent_stroops(), 0);
        assert!(report.to_markdown().contains("No storage key was recorded"));
    }

    #[test]
    fn totals_reconcile_with_the_model() {
        let mut recommender = StorageOptimizationRecommender::new();
        recommender.record_write("pending", StorageTier::Persistent, 20);
        recommender.record_read("pending", StorageTier::Persistent, 20);
        recommender.record_write("config", StorageTier::Persistent, 12);
        for _ in 0..4 {
            recommender.record_read("config", StorageTier::Persistent, 12);
        }

        let report = recommender.report();
        let model = RentModel::default();

        assert_eq!(
            report.current_rent_stroops(),
            model.rent_stroops(StorageTier::Persistent, 20)
                + model.rent_stroops(StorageTier::Persistent, 12)
        );
        assert_eq!(
            report.projected_rent_stroops(),
            report.current_rent_stroops() - report.projected_saving_stroops()
        );
        assert!(report.projected_saving_stroops() > 0);
        assert!(report.projected_saving_percent() > 0.0);
        assert!(report.to_string().contains("Projected rent:"));
    }

    #[test]
    fn temporary_and_instance_rent_is_lower_than_persistent() {
        let model = RentModel::default();

        let persistent = model.rent_stroops(StorageTier::Persistent, 64);
        let temporary = model.rent_stroops(StorageTier::Temporary, 64);
        let instance = model.rent_stroops(StorageTier::Instance, 64);

        assert!(temporary < persistent);
        assert!(instance < persistent);
        assert_eq!(temporary, persistent / 2);
        assert!(model.rent_stroops(StorageTier::Persistent, 0) > 0);
        assert_eq!(model.overhead_bytes(StorageTier::Instance), 0);

        let cheaper = model.with_stroops_per_byte_ledger_micros(1_000);
        assert!(cheaper.rent_stroops(StorageTier::Persistent, 64) < persistent);
        assert_eq!(
            model
                .with_ttl_ledgers(StorageTier::Temporary, 100)
                .ttl_ledgers(StorageTier::Temporary),
            100
        );
    }

    #[test]
    fn access_counts_first_and_last_position() {
        let mut recommender = StorageOptimizationRecommender::new();
        recommender.record_write("pending", StorageTier::Persistent, 20);
        recommender.record_read("config", StorageTier::Persistent, 12);
        recommender.record_read("pending", StorageTier::Persistent, 20);

        let report = recommender.report();
        let pending = report
            .accesses()
            .iter()
            .find(|access| access.key == "pending")
            .expect("pending was tracked");

        assert_eq!(pending.writes, 1);
        assert_eq!(pending.reads, 1);
        assert_eq!(pending.accesses(), 2);
        assert_eq!(pending.qualified(), "persistent:pending");
        assert_eq!(pending.first_seen, 0);
        assert_eq!(pending.last_seen, 2);
        assert_eq!(report.operations(), 3);
        assert_eq!(report.thresholds().hot_reads, 3);
        assert_eq!(report.model().persistent_ttl_ledgers, 3_110_400);
    }

    #[test]
    fn a_write_that_changes_the_tier_moves_the_tracked_tier() {
        let mut recommender = StorageOptimizationRecommender::new();
        recommender.record_read("session", StorageTier::Persistent, 20);
        recommender.record_write("session", StorageTier::Temporary, 20);

        let report = recommender.report();
        let session = &report.accesses()[0];
        assert_eq!(session.tier, StorageTier::Temporary);
        assert_eq!(session.reads, 1);
        assert_eq!(session.writes, 1);
    }

    #[test]
    fn escrow_snapshot_supplies_the_captured_keys() {
        let (env, contract) = escrow();
        let snapshot = StorageSnapshot::capture(&env, &contract);
        let mut recommender = StorageOptimizationRecommender::new();

        recommender.record_reads_from(&snapshot);
        let report = recommender.report();

        let keys: Vec<&str> = report
            .accesses()
            .iter()
            .map(|access| access.key.as_str())
            .collect();
        assert_eq!(keys, vec!["config", "pending"]);
        assert_eq!(recommender.keys().len(), 2);
        assert_eq!(report.thresholds(), AccessThresholds::default());
    }
}
