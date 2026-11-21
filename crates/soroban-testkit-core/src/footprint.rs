//! Storage footprints: how many bytes a contract keeps in each storage tier,
//! recorded in a JSON file a repository commits, and diffed between two
//! revisions so a pull request that inflates state shows up in review.
//!
//! The pieces line up with a CI job:
//!
//! 1. A test captures a [`FootprintSnapshot`] for each contract it cares about
//!    and [`StorageFootprint::record`]s it under a stable label.
//! 2. [`StorageFootprint::save`] writes the artifact; the base branch's copy is
//!    the committed baseline.
//! 3. [`StorageFootprintDiff::compare`] (or the `storage-footprint-diff`
//!    binary) diffs the base artifact against the head one, renders a Markdown
//!    table for a pull-request comment, and reports whether any contract grew
//!    past a percentage threshold.
//!
//! A footprint also projects forward: [`FootprintSnapshot::projected_rent`]
//! prices keeping those bytes on the ledger for another N ledgers, and
//! [`FootprintSnapshot::to_markdown`] renders the entry sizes and that projection
//! as one summary table. The arithmetic is the network's own — it delegates to
//! `soroban_env_host::fees::compute_rent_fee`, so a projection agrees with what
//! core charges rather than with a restatement of the formula here.
//!
//! Sizes are XDR-encoded bytes, the unit rent and read/write fees are charged
//! in: the whole ledger entry for a persistent or temporary key, and the
//! key-plus-value map entry for each key inside instance storage.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use soroban_sdk::xdr::{ContractDataDurability, LedgerEntryData, LedgerKey, Limits, ScVal};
use soroban_sdk::{Address, Env};

use crate::storage::{contract_hash, contract_hash_of, render, StorageTier};

/// The artifact format version this testkit writes and understands.
const FOOTPRINT_VERSION: u32 = 1;

/// Byte sizes of every live storage entry of one contract, keyed `tier:key`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FootprintSnapshot {
    entries: BTreeMap<String, u64>,
}

impl FootprintSnapshot {
    /// Measure `contract`'s live storage in a test environment.
    ///
    /// An address that is not a contract owns no data and measures as empty.
    pub fn capture(env: &Env, contract: &Address) -> Self {
        let mut snapshot = Self::default();
        let Some(wanted) = contract_hash_of(env, contract) else {
            return snapshot;
        };

        for (key, (entry, _live_until)) in env.to_ledger_snapshot().ledger_entries.iter() {
            let LedgerKey::ContractData(lookup) = &**key else {
                continue;
            };
            if contract_hash(&lookup.contract) != Some(wanted) {
                continue;
            }
            let LedgerEntryData::ContractData(stored) = &entry.data else {
                continue;
            };

            match &stored.key {
                ScVal::LedgerKeyContractInstance => {
                    let ScVal::ContractInstance(instance) = &stored.val else {
                        continue;
                    };
                    let Some(storage) = &instance.storage else {
                        continue;
                    };
                    for item in storage.0.iter() {
                        snapshot.insert(StorageTier::Instance, &render(&item.key), xdr_len(item));
                    }
                }
                stored_key => {
                    let tier = match stored.durability {
                        ContractDataDurability::Temporary => StorageTier::Temporary,
                        ContractDataDurability::Persistent => StorageTier::Persistent,
                    };
                    snapshot.insert(tier, &render(stored_key), xdr_len(&**entry));
                }
            }
        }
        snapshot
    }

    /// Record `bytes` for `key` in `tier`, replacing any earlier size.
    pub fn insert(&mut self, tier: StorageTier, key: &str, bytes: u64) -> &mut Self {
        self.entries
            .insert(format!("{}:{}", tier.as_str(), key), bytes);
        self
    }

    /// Every entry as `(qualified key, bytes)`, in key order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, u64)> {
        self.entries
            .iter()
            .map(|(key, bytes)| (key.as_str(), *bytes))
    }

    /// The size of one `tier:key` entry.
    pub fn get(&self, qualified_key: &str) -> Option<u64> {
        self.entries.get(qualified_key).copied()
    }

    /// Total bytes held in one tier.
    pub fn tier_bytes(&self, tier: StorageTier) -> u64 {
        self.entries
            .iter()
            .filter(|(key, _)| tier_of(key) == Some(tier))
            .map(|(_, bytes)| bytes)
            .sum()
    }

    /// Total bytes across all tiers.
    pub fn total_bytes(&self) -> u64 {
        self.entries.values().sum()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Why a footprint artifact could not be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FootprintError {
    Io(String),
    Parse(String),
    UnsupportedVersion { found: u32 },
}

impl std::fmt::Display for FootprintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FootprintError::Io(reason) => write!(f, "cannot read the footprint file: {reason}"),
            FootprintError::Parse(reason) => write!(f, "footprint file is not valid: {reason}"),
            FootprintError::UnsupportedVersion { found } => write!(
                f,
                "footprint file version {found} is newer than this testkit understands ({FOOTPRINT_VERSION})"
            ),
        }
    }
}

impl std::error::Error for FootprintError {}

#[derive(Serialize, Deserialize)]
struct FootprintFile {
    version: u32,
    contracts: BTreeMap<String, FootprintSnapshot>,
}

/// Footprints for a set of contracts, the JSON artifact a repository commits.
///
/// ```json
/// {
///   "version": 1,
///   "contracts": {
///     "token": {
///       "entries": { "instance:admin": 84, "persistent:balances": 212 }
///     }
///   }
/// }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StorageFootprint {
    contracts: BTreeMap<String, FootprintSnapshot>,
}

impl StorageFootprint {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store `snapshot` under `label`, replacing an earlier one.
    pub fn record(&mut self, label: impl Into<String>, snapshot: FootprintSnapshot) -> &mut Self {
        self.contracts.insert(label.into(), snapshot);
        self
    }

    /// Capture `contract` and record it under `label` in one step.
    pub fn capture(
        &mut self,
        label: impl Into<String>,
        env: &Env,
        contract: &Address,
    ) -> &mut Self {
        self.record(label, FootprintSnapshot::capture(env, contract))
    }

    pub fn get(&self, label: &str) -> Option<&FootprintSnapshot> {
        self.contracts.get(label)
    }

    pub fn contracts(&self) -> impl Iterator<Item = (&str, &FootprintSnapshot)> {
        self.contracts
            .iter()
            .map(|(label, snapshot)| (label.as_str(), snapshot))
    }

    pub fn len(&self) -> usize {
        self.contracts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contracts.is_empty()
    }

    pub fn load(path: &std::path::Path) -> Result<Self, FootprintError> {
        let text =
            std::fs::read_to_string(path).map_err(|error| FootprintError::Io(error.to_string()))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, FootprintError> {
        let file: FootprintFile =
            serde_json::from_str(text).map_err(|error| FootprintError::Parse(error.to_string()))?;
        if file.version > FOOTPRINT_VERSION {
            return Err(FootprintError::UnsupportedVersion {
                found: file.version,
            });
        }
        Ok(Self {
            contracts: file.contracts,
        })
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), FootprintError> {
        let mut text = self.to_json_string();
        text.push('\n');
        std::fs::write(path, text.as_bytes()).map_err(|error| FootprintError::Io(error.to_string()))
    }

    pub fn to_json_string(&self) -> String {
        let file = FootprintFile {
            version: FOOTPRINT_VERSION,
            contracts: self.contracts.clone(),
        };
        // A version integer and nested string-to-u64 maps always serialise.
        serde_json::to_string_pretty(&file).unwrap_or_default()
    }

    /// Measure one contract without assembling an artifact around it.
    ///
    /// [`StorageFootprint::capture`] does the same work in order to *record* a
    /// snapshot under a label; this is for a test that only wants to read the
    /// sizes and the rent of a single contract.
    pub fn measure(env: &Env, contract: &Address) -> FootprintSnapshot {
        FootprintSnapshot::capture(env, contract)
    }
}

/// Horizon [`FootprintSnapshot::to_markdown`] projects rent over when the caller
/// does not name one: 30 days at the network's ~5 second ledger cadence.
pub const DEFAULT_PROJECTION_LEDGERS: u32 = 518_400;

/// The state size at which `fee_per_rent_1kb` reaches its high value.
const PROTOCOL_23_STATE_TARGET_SIZE_BYTES: i64 = 3_000_000_000;
/// `fee_per_rent_1kb` extrapolated to an empty Soroban state; the host floors
/// the result at `MINIMUM_RENT_WRITE_FEE_PER_1KB`.
const PROTOCOL_23_RENT_FEE_1KB_STATE_SIZE_LOW: i64 = -17_000;
/// `fee_per_rent_1kb` once the Soroban state reaches the target size.
const PROTOCOL_23_RENT_FEE_1KB_STATE_SIZE_HIGH: i64 = 10_000;
/// Multiplier applied to the growth of `fee_per_rent_1kb` past the target size.
const PROTOCOL_23_STATE_SIZE_RENT_FEE_GROWTH_FACTOR: u32 = 1;
/// The floor the host clamps `fee_per_rent_1kb` to, whatever the state size.
const MINIMUM_RENT_WRITE_FEE_PER_1KB: i64 = 1_000;

/// The fee parameters a rent projection is priced with.
///
/// The defaults are the values `stellar-core` writes into the ledger at the
/// protocol 23 upgrade — `Protcol23UpgradedConfig` in `src/ledger/NetworkConfig.h`
/// — which that header notes are "calibrated for the pubnet, but they are also
/// suitable for the smaller test networks", so mainnet and testnet price a byte
/// of rent identically today.
///
/// Two of them move with the network rather than with the protocol:
/// `fee_per_rent_1kb` follows the live Soroban state size, which
/// [`RentParameters::with_soroban_state_size`] applies with core's own curve, and
/// `fee_per_write_1kb` follows the bucket list size. Both default to the floor
/// the respective curve returns while the network is smaller than its target,
/// which keeps a projection a lower bound rather than a guess.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RentParameters {
    /// Stroops per 1 KB of ledger state written, including a TTL bump.
    pub fee_per_write_1kb: i64,
    /// Stroops per 1 KB of rented ledger state per day.
    pub fee_per_rent_1kb: i64,
    /// Stroops per ledger entry written.
    pub fee_per_write_entry: i64,
    /// Ledgers one `fee_per_rent_1kb` payment covers for a persistent entry.
    pub persistent_rent_rate_denominator: i64,
    /// The same for a temporary entry, which the network rents more cheaply.
    pub temporary_rent_rate_denominator: i64,
}

impl Default for RentParameters {
    fn default() -> Self {
        Self {
            fee_per_write_1kb: MINIMUM_RENT_WRITE_FEE_PER_1KB,
            fee_per_rent_1kb: MINIMUM_RENT_WRITE_FEE_PER_1KB,
            fee_per_write_entry: 20_000,
            persistent_rent_rate_denominator: 1_215,
            temporary_rent_rate_denominator: 2_430,
        }
    }
}

impl RentParameters {
    /// Re-derive `fee_per_rent_1kb` for a network holding `bytes` of Soroban
    /// state, using the curve `stellar-core` itself applies.
    ///
    /// Pass the `sorobanStateSize` a `getNetworkSettings` call reports to price
    /// against the live network instead of the floor.
    pub fn with_soroban_state_size(mut self, bytes: i64) -> Self {
        self.fee_per_rent_1kb = soroban_env_host::fees::compute_rent_write_fee_per_1kb(
            bytes,
            &soroban_env_host::fees::RentWriteFeeConfiguration {
                state_target_size_bytes: PROTOCOL_23_STATE_TARGET_SIZE_BYTES,
                rent_fee_1kb_state_size_low: PROTOCOL_23_RENT_FEE_1KB_STATE_SIZE_LOW,
                rent_fee_1kb_state_size_high: PROTOCOL_23_RENT_FEE_1KB_STATE_SIZE_HIGH,
                state_size_rent_fee_growth_factor: PROTOCOL_23_STATE_SIZE_RENT_FEE_GROWTH_FACTOR,
            },
        );
        self
    }

    fn host_config(&self) -> soroban_env_host::fees::RentFeeConfiguration {
        soroban_env_host::fees::RentFeeConfiguration {
            fee_per_write_1kb: self.fee_per_write_1kb,
            fee_per_rent_1kb: self.fee_per_rent_1kb,
            fee_per_write_entry: self.fee_per_write_entry,
            persistent_rent_rate_denominator: self.persistent_rent_rate_denominator,
            temporary_rent_rate_denominator: self.temporary_rent_rate_denominator,
        }
    }
}

/// Stroops one footprint projects to over a horizon, per storage tier.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RentEstimate {
    /// Ledgers of lifetime the entries are priced for.
    pub ledgers: u32,
    /// Instance storage, priced as the single ledger entry that holds it.
    pub instance_stroops: u64,
    pub persistent_stroops: u64,
    pub temporary_stroops: u64,
}

impl RentEstimate {
    pub fn total_stroops(&self) -> u64 {
        self.instance_stroops
            .saturating_add(self.persistent_stroops)
            .saturating_add(self.temporary_stroops)
    }

    fn tier(&self, tier: StorageTier) -> u64 {
        match tier {
            StorageTier::Instance => self.instance_stroops,
            StorageTier::Persistent => self.persistent_stroops,
            StorageTier::Temporary => self.temporary_stroops,
        }
    }
}

impl FootprintSnapshot {
    /// Stroops it costs to hold every byte of this footprint for `ledgers` more
    /// ledgers, at the protocol 23 fee parameters.
    pub fn projected_rent(&self, ledgers: u32) -> u64 {
        self.rent_estimate(&RentParameters::default(), ledgers)
            .total_stroops()
    }

    /// The same projection against caller-chosen fee parameters.
    ///
    /// Each entry is priced the way core prices a TTL bump: `ledgers` of rent at
    /// the entry's size, plus one entry write, plus the 48-byte `TtlEntry` the
    /// bump itself writes. So the figure is what keeping the state alive costs,
    /// not just the rent term of it.
    ///
    /// Instance storage is one `ContractData` ledger entry holding a map, so its
    /// bytes are priced as a single persistent entry rather than one per key. The
    /// snapshot records the key-plus-value map entries and not the entry envelope
    /// around them, which makes the instance column a floor.
    pub fn rent_estimate(&self, params: &RentParameters, ledgers: u32) -> RentEstimate {
        let mut estimate = RentEstimate {
            ledgers,
            ..RentEstimate::default()
        };
        let instance: Vec<u64> = self
            .entries_with_tier(StorageTier::Instance)
            .map(|(_, bytes)| bytes)
            .collect();
        if !instance.is_empty() {
            estimate.instance_stroops = entry_rent(params, true, instance.iter().sum(), ledgers);
        }
        for (tier, is_persistent) in [
            (StorageTier::Persistent, true),
            (StorageTier::Temporary, false),
        ] {
            for (_, bytes) in self.entries_with_tier(tier) {
                let stroops = entry_rent(params, is_persistent, bytes, ledgers);
                let total = match tier {
                    StorageTier::Persistent => &mut estimate.persistent_stroops,
                    _ => &mut estimate.temporary_stroops,
                };
                *total = total.saturating_add(stroops);
            }
        }
        estimate
    }

    fn entries_with_tier(&self, tier: StorageTier) -> impl Iterator<Item = (&str, u64)> {
        self.entries()
            .filter(move |(key, _)| tier_of(key) == Some(tier))
    }

    /// A Markdown summary of what this contract stores and what it costs to keep.
    ///
    /// One row per entry with its XDR size, then the tier subtotals with the
    /// [`DEFAULT_PROJECTION_LEDGERS`] rent each tier projects to.
    pub fn to_markdown(&self) -> String {
        self.to_markdown_with(&RentParameters::default(), DEFAULT_PROJECTION_LEDGERS)
    }

    /// [`FootprintSnapshot::to_markdown`] over a caller-named horizon and fee set.
    pub fn to_markdown_with(&self, params: &RentParameters, ledgers: u32) -> String {
        let estimate = self.rent_estimate(params, ledgers);
        let mut out = String::from("### Entries\n\n");
        out.push_str("| Key | Bytes |\n|-----|--------:|\n");
        for (key, bytes) in self.entries() {
            let _ = writeln!(out, "| `{key}` | {bytes} |");
        }
        if self.is_empty() {
            let _ = writeln!(out, "| _none_ | 0 |");
        }

        let _ = writeln!(
            out,
            "\n| Tier | Entries | Bytes | Rent for {ledgers} ledgers |\n\
             |------|--------:|------:|----------------------:|"
        );
        for tier in TIERS {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                tier.as_str(),
                self.entries_with_tier(tier).count(),
                self.tier_bytes(tier),
                estimate.tier(tier)
            );
        }
        let _ = writeln!(
            out,
            "| **Total** | **{}** | **{}** | **{}** |",
            self.len(),
            self.total_bytes(),
            estimate.total_stroops()
        );
        out.push_str(
            "\n<sub>Sizes are XDR-encoded bytes. Rent is priced by \
             `soroban-env-host`'s `compute_rent_fee` at the protocol 23 fee \
             parameters, entry write and `TtlEntry` included.</sub>\n",
        );
        out
    }
}

/// Price one ledger entry of `bytes` for `ledgers`, through the host's own
/// fee function.
fn entry_rent(params: &RentParameters, is_persistent: bool, bytes: u64, ledgers: u32) -> u64 {
    let size = u32::try_from(bytes).unwrap_or(u32::MAX);
    // An unchanged size and a `0 -> ledgers` lifetime is how core spells "buy
    // this entry `ledgers` more of them": `old_size == new_size` leaves the
    // top-up term at zero and the whole horizon lands in the extension.
    let change = soroban_env_host::fees::LedgerEntryRentChange {
        is_persistent,
        is_code_entry: false,
        old_size_bytes: size,
        new_size_bytes: size,
        old_live_until_ledger: 0,
        new_live_until_ledger: ledgers,
    };
    soroban_env_host::fees::compute_rent_fee(&[change], &params.host_config(), 0).max(0) as u64
}

impl StorageFootprint {
    /// Stroops every recorded contract costs to hold for `ledgers` more ledgers.
    pub fn projected_rent(&self, ledgers: u32) -> u64 {
        self.contracts
            .values()
            .map(|snapshot| snapshot.projected_rent(ledgers))
            .sum()
    }

    /// Every recorded contract as one Markdown report: [`FootprintSnapshot`]
    /// tables back to back under a contract heading.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        for (label, snapshot) in self.contracts() {
            let _ = writeln!(out, "## `{label}`\n");
            out.push_str(&snapshot.to_markdown());
            out.push('\n');
        }
        out
    }
}

/// One storage key whose size differs between two footprints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyChange {
    pub contract: String,
    /// `tier:key`.
    pub key: String,
    /// `None` when the key is new in the head footprint.
    pub base_bytes: Option<u64>,
    /// `None` when the key was removed in the head footprint.
    pub head_bytes: Option<u64>,
}

impl KeyChange {
    pub fn delta(&self) -> i128 {
        i128::from(self.head_bytes.unwrap_or(0)) - i128::from(self.base_bytes.unwrap_or(0))
    }
}

/// Total size of one contract in the base and head footprints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractChange {
    pub contract: String,
    /// `None` when the contract only exists in the head footprint.
    pub base: Option<[u64; 3]>,
    /// `None` when the contract was dropped from the head footprint.
    pub head: Option<[u64; 3]>,
}

const TIERS: [StorageTier; 3] = [
    StorageTier::Instance,
    StorageTier::Persistent,
    StorageTier::Temporary,
];

impl ContractChange {
    pub fn base_total(&self) -> u64 {
        self.base.map(|tiers| tiers.iter().sum()).unwrap_or(0)
    }

    pub fn head_total(&self) -> u64 {
        self.head.map(|tiers| tiers.iter().sum()).unwrap_or(0)
    }

    pub fn delta(&self) -> i128 {
        i128::from(self.head_total()) - i128::from(self.base_total())
    }

    /// Growth of the contract's total footprint as a percentage of the base.
    ///
    /// `None` when there is nothing to grow from: the contract is new, or its
    /// base footprint was empty.
    pub fn growth_percent(&self) -> Option<f64> {
        let base = self.base_total();
        (self.base.is_some() && base > 0).then(|| self.delta() as f64 * 100.0 / base as f64)
    }
}

/// The comparison of a base footprint artifact against a head one.
#[derive(Clone, Debug, PartialEq)]
pub struct StorageFootprintDiff {
    contracts: Vec<ContractChange>,
    keys: Vec<KeyChange>,
}

impl StorageFootprintDiff {
    /// Diff every contract in either artifact, key by key.
    pub fn compare(base: &StorageFootprint, head: &StorageFootprint) -> Self {
        let labels: std::collections::BTreeSet<&str> = base
            .contracts
            .keys()
            .chain(head.contracts.keys())
            .map(String::as_str)
            .collect();

        let mut contracts = Vec::new();
        let mut keys = Vec::new();
        for label in labels {
            let before = base.get(label);
            let after = head.get(label);
            let totals = |snapshot: &FootprintSnapshot| TIERS.map(|tier| snapshot.tier_bytes(tier));
            contracts.push(ContractChange {
                contract: label.to_string(),
                base: before.map(totals),
                head: after.map(totals),
            });

            let empty = FootprintSnapshot::default();
            let (before, after) = (before.unwrap_or(&empty), after.unwrap_or(&empty));
            let names: std::collections::BTreeSet<&str> = before
                .entries
                .keys()
                .chain(after.entries.keys())
                .map(String::as_str)
                .collect();
            for name in names {
                let (base_bytes, head_bytes) = (before.get(name), after.get(name));
                if base_bytes != head_bytes {
                    keys.push(KeyChange {
                        contract: label.to_string(),
                        key: name.to_string(),
                        base_bytes,
                        head_bytes,
                    });
                }
            }
        }
        // Largest growth first, so the keys worth reading head the table.
        keys.sort_by(|a, b| b.delta().cmp(&a.delta()).then_with(|| a.key.cmp(&b.key)));

        Self { contracts, keys }
    }

    pub fn contracts(&self) -> &[ContractChange] {
        &self.contracts
    }

    /// Every key whose size changed, largest growth first.
    pub fn changed_keys(&self) -> &[KeyChange] {
        &self.keys
    }

    /// Keys that are new or larger in the head footprint.
    pub fn growing_keys(&self) -> impl Iterator<Item = &KeyChange> {
        self.keys.iter().filter(|change| change.delta() > 0)
    }

    pub fn is_unchanged(&self) -> bool {
        self.keys.is_empty()
            && self
                .contracts
                .iter()
                .all(|c| c.base.is_some() == c.head.is_some())
    }

    /// Contracts whose total footprint grew by more than `threshold_percent`.
    ///
    /// A contract with no base to grow from (new, or previously empty) is not
    /// counted: it has no percentage, and adding storage is the point of a new
    /// contract.
    pub fn exceeding(&self, threshold_percent: f64) -> Vec<&ContractChange> {
        self.contracts
            .iter()
            .filter(|change| {
                change
                    .growth_percent()
                    .is_some_and(|growth| growth > threshold_percent)
            })
            .collect()
    }

    /// Render the comparison as GitHub-flavoured Markdown for a PR comment.
    ///
    /// With a threshold, the report opens with a pass/fail verdict naming the
    /// contracts over it.
    pub fn to_markdown(&self, threshold_percent: Option<f64>) -> String {
        let mut out = String::from("## Storage footprint\n\n");

        if let Some(threshold) = threshold_percent {
            let over = self.exceeding(threshold);
            if over.is_empty() {
                let _ = writeln!(
                    out,
                    "**Pass** — no contract's storage grew by more than {}%.\n",
                    trim(threshold)
                );
            } else {
                let names: Vec<String> = over
                    .iter()
                    .map(|change| {
                        format!(
                            "`{}` (+{}%)",
                            change.contract,
                            trim(change.growth_percent().unwrap_or(0.0))
                        )
                    })
                    .collect();
                let _ = writeln!(
                    out,
                    "**Fail** — storage grew by more than {}% in {}.\n",
                    trim(threshold),
                    names.join(", ")
                );
            }
        }

        out.push_str("| Contract | Instance | Persistent | Temporary | Total | Change |\n");
        out.push_str("|----------|---------:|-----------:|----------:|------:|-------:|\n");
        for change in &self.contracts {
            let cells: Vec<String> = (0..3)
                .map(|tier| bytes_cell(change.base.map(|t| t[tier]), change.head.map(|t| t[tier])))
                .collect();
            let status = match (change.base.is_some(), change.head.is_some()) {
                (false, true) => "new".to_string(),
                (true, false) => "removed".to_string(),
                _ => match change.growth_percent() {
                    Some(growth) => percent(growth),
                    None => signed(change.delta()),
                },
            };
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} | {} | {} |",
                change.contract,
                cells[0],
                cells[1],
                cells[2],
                bytes_cell(
                    change.base.map(|_| change.base_total()),
                    change.head.map(|_| change.head_total())
                ),
                status
            );
        }

        if self.keys.is_empty() {
            out.push_str("\nNo storage key changed size.\n");
        } else {
            out.push_str("\n### Changed keys\n\n");
            out.push_str("| Contract | Key | Base | Head | Change |\n");
            out.push_str("|----------|-----|-----:|-----:|-------:|\n");
            for change in &self.keys {
                let marker = match (change.base_bytes, change.head_bytes) {
                    (None, _) => " (new)",
                    (_, None) => " (removed)",
                    _ => "",
                };
                let _ = writeln!(
                    out,
                    "| `{}` | `{}` | {} | {} | {}{} |",
                    change.contract,
                    change.key,
                    optional(change.base_bytes),
                    optional(change.head_bytes),
                    if change.delta() > 0 {
                        format!("**{}**", signed(change.delta()))
                    } else {
                        signed(change.delta())
                    },
                    marker
                );
            }
        }
        out.push_str("\n<sub>Sizes are XDR-encoded bytes.</sub>\n");
        out
    }
}

/// Usage text for the `storage-footprint-diff` binary.
pub const DIFF_CLI_USAGE: &str = "\
usage: storage-footprint-diff BASE.json HEAD.json [--threshold PCT] [--output FILE] [--allow-missing-base]

Compares two storage footprint artifacts and prints a Markdown report.

  --threshold PCT        exit 1 when any contract's total footprint grew by more than PCT percent
  --output FILE          also write the Markdown report to FILE (for a PR comment step)
  --allow-missing-base   treat a missing BASE file as an empty footprint, for the first run

Exit status: 0 within threshold, 1 threshold exceeded, 2 usage or file error.
";

/// What the `storage-footprint-diff` binary prints and returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffCliOutcome {
    pub stdout: String,
    pub stderr: String,
    /// `0` within threshold, `1` threshold exceeded, `2` usage or file error.
    pub exit_code: u8,
}

/// The `storage-footprint-diff` binary, as a function, so it can be tested
/// and embedded without spawning a process.
pub fn run_diff_cli(args: impl IntoIterator<Item = String>) -> DiffCliOutcome {
    match diff_cli(args.into_iter().collect()) {
        Ok(outcome) => outcome,
        Err(message) => DiffCliOutcome {
            stdout: String::new(),
            stderr: format!("storage-footprint-diff: {message}\n\n{DIFF_CLI_USAGE}"),
            exit_code: 2,
        },
    }
}

fn diff_cli(args: Vec<String>) -> Result<DiffCliOutcome, String> {
    let mut paths = Vec::new();
    let mut threshold = None;
    let mut output = None;
    let mut allow_missing_base = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                return Ok(DiffCliOutcome {
                    stdout: DIFF_CLI_USAGE.to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                })
            }
            "--threshold" => {
                let value = args.next().ok_or("--threshold needs a percentage")?;
                let parsed: f64 = value
                    .trim_end_matches('%')
                    .parse()
                    .map_err(|_| format!("--threshold expects a number, got {value:?}"))?;
                if !parsed.is_finite() || parsed < 0.0 {
                    return Err(format!(
                        "--threshold must be a non-negative number, got {value:?}"
                    ));
                }
                threshold = Some(parsed);
            }
            "--output" => output = Some(args.next().ok_or("--output needs a file path")?),
            "--allow-missing-base" => allow_missing_base = true,
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ => paths.push(arg),
        }
    }
    let [base_path, head_path] = <[String; 2]>::try_from(paths)
        .map_err(|paths| format!("expected BASE and HEAD files, got {} path(s)", paths.len()))?;

    let base_path = std::path::Path::new(&base_path);
    let base = if allow_missing_base && !base_path.exists() {
        StorageFootprint::new()
    } else {
        StorageFootprint::load(base_path).map_err(|e| format!("{}: {e}", base_path.display()))?
    };
    let head = StorageFootprint::load(std::path::Path::new(&head_path))
        .map_err(|e| format!("{head_path}: {e}"))?;

    let diff = StorageFootprintDiff::compare(&base, &head);
    let markdown = diff.to_markdown(threshold);
    if let Some(output) = output {
        std::fs::write(&output, markdown.as_bytes())
            .map_err(|e| format!("cannot write {output}: {e}"))?;
    }

    let over = threshold.map(|t| diff.exceeding(t).len()).unwrap_or(0);
    Ok(DiffCliOutcome {
        stdout: markdown,
        stderr: if over > 0 {
            format!(
                "storage-footprint-diff: {over} contract(s) grew past the {}% threshold\n",
                trim(threshold.unwrap_or(0.0))
            )
        } else {
            String::new()
        },
        exit_code: u8::from(over > 0),
    })
}

fn tier_of(qualified: &str) -> Option<StorageTier> {
    let (tier, _) = qualified.split_once(':')?;
    TIERS
        .into_iter()
        .find(|candidate| candidate.as_str() == tier)
}

fn xdr_len(value: &impl soroban_sdk::xdr::WriteXdr) -> u64 {
    value
        .to_xdr(Limits::none())
        .map(|bytes| bytes.len() as u64)
        .unwrap_or(0)
}

fn bytes_cell(base: Option<u64>, head: Option<u64>) -> String {
    match (base, head) {
        (Some(base), Some(head)) if base == head => head.to_string(),
        (base, head) => format!("{} → {}", optional(base), optional(head)),
    }
}

fn optional(bytes: Option<u64>) -> String {
    bytes.map_or_else(|| "–".to_string(), |bytes| bytes.to_string())
}

fn signed(delta: i128) -> String {
    if delta > 0 {
        format!("+{delta} B")
    } else {
        format!("{delta} B")
    }
}

fn percent(growth: f64) -> String {
    if growth > 0.0 {
        format!("+{}%", trim(growth))
    } else {
        format!("{}%", trim(growth))
    }
}

/// `10`, `2.5` and `33.33` rather than `10.00`.
fn trim(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        FootprintError, FootprintSnapshot, RentParameters, StorageFootprint, StorageFootprintDiff,
        DEFAULT_PROJECTION_LEDGERS, FOOTPRINT_VERSION,
    };
    use crate::storage::StorageTier;
    use soroban_sdk::{contract, contractimpl, symbol_short, Bytes, Env, Symbol};

    #[contract]
    pub struct Store;

    #[contractimpl]
    impl Store {
        pub fn seed(env: Env) {
            env.storage().instance().set(&symbol_short!("admin"), &7u32);
            env.storage()
                .persistent()
                .set(&symbol_short!("count"), &42u32);
            env.storage()
                .temporary()
                .set(&symbol_short!("nonce"), &1u32);
        }

        pub fn write_blob(env: Env, len: u32) {
            let blob = Bytes::from_slice(&env, &vec![0xAB; len as usize]);
            env.storage()
                .persistent()
                .set(&Symbol::new(&env, "blob"), &blob);
        }
    }

    fn seeded(blob: Option<u32>) -> FootprintSnapshot {
        let env = Env::default();
        let id = env.register(Store, ());
        let client = StoreClient::new(&env, &id);
        client.seed();
        if let Some(len) = blob {
            client.write_blob(&len);
        }
        FootprintSnapshot::capture(&env, &id)
    }

    fn footprint(snapshot: FootprintSnapshot) -> StorageFootprint {
        let mut footprint = StorageFootprint::new();
        footprint.record("store", snapshot);
        footprint
    }

    #[test]
    fn capture_measures_every_tier_in_xdr_bytes() {
        let snapshot = seeded(None);

        let keys: Vec<&str> = snapshot.entries().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            vec!["instance:admin", "persistent:count", "temporary:nonce"]
        );
        for tier in [
            StorageTier::Instance,
            StorageTier::Persistent,
            StorageTier::Temporary,
        ] {
            assert!(snapshot.tier_bytes(tier) > 0, "{tier:?} measured empty");
        }
        assert_eq!(
            snapshot.total_bytes(),
            snapshot.entries().map(|(_, bytes)| bytes).sum::<u64>()
        );
    }

    #[test]
    fn a_larger_value_measures_larger() {
        let small = seeded(Some(12));
        let large = seeded(Some(1_000));

        let grew = large.get("persistent:blob").unwrap() - small.get("persistent:blob").unwrap();
        assert_eq!(grew, 988);
    }

    #[test]
    fn the_artifact_round_trips_through_json() {
        let mut artifact = footprint(seeded(Some(32)));
        artifact.record("empty", FootprintSnapshot::default());

        let text = artifact.to_json_string();
        assert!(text.contains("\"version\": 1"));
        assert!(text.contains("\"persistent:blob\""));
        assert_eq!(StorageFootprint::parse(&text).unwrap(), artifact);
    }

    #[test]
    fn a_newer_artifact_is_refused() {
        let text = format!(
            r#"{{"version": {}, "contracts": {{}}}}"#,
            FOOTPRINT_VERSION + 1
        );

        assert_eq!(
            StorageFootprint::parse(&text).unwrap_err(),
            FootprintError::UnsupportedVersion {
                found: FOOTPRINT_VERSION + 1
            }
        );
    }

    #[test]
    fn save_and_load_use_the_same_format() {
        let artifact = footprint(seeded(None));
        let path = std::env::temp_dir().join(format!(
            "soroban-testkit-footprint-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        artifact.save(&path).unwrap();
        let loaded = StorageFootprint::load(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.unwrap(), artifact);
    }

    #[test]
    fn identical_footprints_diff_to_nothing() {
        let base = footprint(seeded(Some(64)));
        let diff = StorageFootprintDiff::compare(&base, &base.clone());

        assert!(diff.is_unchanged());
        assert!(diff.exceeding(0.0).is_empty());
        assert!(diff
            .to_markdown(Some(5.0))
            .contains("No storage key changed size."));
    }

    #[test]
    fn a_growing_key_is_highlighted_first() {
        let base = footprint(seeded(Some(16)));
        let head = footprint(seeded(Some(400)));

        let diff = StorageFootprintDiff::compare(&base, &head);
        let top = &diff.changed_keys()[0];

        assert_eq!(top.key, "persistent:blob");
        assert_eq!(top.delta(), 384);
        assert_eq!(diff.growing_keys().count(), 1);
    }

    #[test]
    fn new_and_removed_keys_are_reported() {
        let base = footprint(seeded(None));
        let head = footprint(seeded(Some(8)));

        let added = StorageFootprintDiff::compare(&base, &head);
        assert_eq!(added.changed_keys()[0].base_bytes, None);
        assert!(added.to_markdown(None).contains("(new)"));

        let removed = StorageFootprintDiff::compare(&head, &base);
        assert_eq!(removed.changed_keys()[0].head_bytes, None);
        assert!(removed.to_markdown(None).contains("(removed)"));
    }

    #[test]
    fn the_threshold_fails_only_past_the_configured_growth() {
        let mut base = FootprintSnapshot::default();
        base.insert(StorageTier::Persistent, "balances", 1_000);
        let mut head = base.clone();
        head.insert(StorageTier::Persistent, "balances", 1_100);

        let diff = StorageFootprintDiff::compare(&footprint(base), &footprint(head));
        assert_eq!(diff.contracts()[0].growth_percent(), Some(10.0));

        assert!(
            diff.exceeding(10.0).is_empty(),
            "exactly at the threshold passes"
        );
        assert_eq!(diff.exceeding(9.9).len(), 1);
        assert!(diff
            .to_markdown(Some(10.0))
            .starts_with("## Storage footprint\n\n**Pass**"));
        assert!(diff
            .to_markdown(Some(5.0))
            .contains("**Fail** — storage grew by more than 5% in `store` (+10%)."));
    }

    #[test]
    fn shrinking_storage_never_fails() {
        let mut base = FootprintSnapshot::default();
        base.insert(StorageTier::Instance, "config", 500);
        let mut head = FootprintSnapshot::default();
        head.insert(StorageTier::Instance, "config", 100);

        let diff = StorageFootprintDiff::compare(&footprint(base), &footprint(head));

        assert!(diff.exceeding(0.0).is_empty());
        assert!(diff.to_markdown(None).contains("| -80% |"));
    }

    #[test]
    fn a_new_contract_is_reported_but_not_counted_against_the_threshold() {
        let base = StorageFootprint::new();
        let head = footprint(seeded(None));

        let diff = StorageFootprintDiff::compare(&base, &head);

        assert!(diff.exceeding(0.0).is_empty());
        assert_eq!(diff.contracts()[0].growth_percent(), None);
        assert!(diff.to_markdown(Some(1.0)).contains("| new |"));
    }

    #[test]
    fn the_markdown_table_has_one_row_per_contract_and_tier_columns() {
        let mut base = StorageFootprint::new();
        let mut a = FootprintSnapshot::default();
        a.insert(StorageTier::Instance, "admin", 40)
            .insert(StorageTier::Persistent, "count", 60);
        base.record("alpha", a.clone());
        base.record("beta", a.clone());
        let mut head = base.clone();
        a.insert(StorageTier::Persistent, "count", 90);
        head.record("alpha", a);

        let markdown = StorageFootprintDiff::compare(&base, &head).to_markdown(Some(10.0));

        assert!(
            markdown.contains("| Contract | Instance | Persistent | Temporary | Total | Change |")
        );
        assert!(markdown.contains("| `alpha` | 40 | 60 → 90 | 0 | 100 → 130 | +30% |"));
        assert!(markdown.contains("| `beta` | 40 | 60 | 0 | 100 | 0% |"));
        assert!(markdown.contains("| `alpha` | `persistent:count` | 60 | 90 | **+30 B** |"));
        assert!(markdown.contains("**Fail**"));
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "soroban-testkit-{name}-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn artifact_with(bytes: u64) -> StorageFootprint {
        let mut snapshot = FootprintSnapshot::default();
        snapshot.insert(StorageTier::Persistent, "balances", bytes);
        footprint(snapshot)
    }

    fn cli(args: &[&str]) -> super::DiffCliOutcome {
        super::run_diff_cli(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn the_cli_fails_ci_past_the_threshold_and_writes_the_report() {
        let (base, head, report) = (temp_path("base"), temp_path("head"), temp_path("report"));
        artifact_with(1_000).save(&base).unwrap();
        artifact_with(1_250).save(&head).unwrap();
        let (b, h, r) = (
            base.to_str().unwrap(),
            head.to_str().unwrap(),
            report.to_str().unwrap(),
        );

        let pass = cli(&[b, h, "--threshold", "30"]);
        let fail = cli(&[b, h, "--threshold", "10%", "--output", r]);
        let written = std::fs::read_to_string(&report);
        for path in [&base, &head, &report] {
            let _ = std::fs::remove_file(path);
        }

        assert_eq!(pass.exit_code, 0, "{}", pass.stderr);
        assert!(pass.stdout.contains("**Pass**"));
        assert_eq!(fail.exit_code, 1);
        assert!(fail
            .stderr
            .contains("1 contract(s) grew past the 10% threshold"));
        assert_eq!(written.unwrap(), fail.stdout);
    }

    #[test]
    fn the_cli_reports_usage_errors_with_status_2() {
        assert_eq!(cli(&[]).exit_code, 2);
        assert!(cli(&["only-one.json"])
            .stderr
            .contains("expected BASE and HEAD files"));
        assert!(cli(&["a", "b", "--threshold", "lots"])
            .stderr
            .contains("expects a number"));
        assert!(cli(&["a", "b", "--frobnicate"])
            .stderr
            .contains("unknown option --frobnicate"));
        assert!(cli(&["missing-base.json", "missing-head.json"])
            .stderr
            .contains("missing-base.json"));
        assert_eq!(cli(&["--help"]).exit_code, 0);
    }

    #[test]
    fn a_missing_base_can_be_allowed_on_the_first_run() {
        let head = temp_path("first-head");
        artifact_with(500).save(&head).unwrap();
        let h = head.to_str().unwrap();

        let refused = cli(&["does-not-exist.json", h]);
        let allowed = cli(&[
            "does-not-exist.json",
            h,
            "--allow-missing-base",
            "--threshold",
            "0",
        ]);
        let _ = std::fs::remove_file(&head);

        assert_eq!(refused.exit_code, 2);
        assert_eq!(allowed.exit_code, 0, "{}", allowed.stderr);
        assert!(allowed.stdout.contains("| new |"));
    }

    /// One entry of every tier, with sizes chosen so no two tiers share a total.
    fn priced() -> FootprintSnapshot {
        let mut snapshot = FootprintSnapshot::default();
        snapshot
            .insert(StorageTier::Instance, "admin", 40)
            .insert(StorageTier::Instance, "config", 60)
            .insert(StorageTier::Persistent, "count", 100)
            .insert(StorageTier::Persistent, "blob", 300)
            .insert(StorageTier::Temporary, "nonce", 80);
        snapshot
    }

    fn ceil_div(numerator: i128, denominator: i128) -> i128 {
        (numerator + denominator - 1) / denominator
    }

    /// The protocol's rent arithmetic, restated here independently of
    /// [`super::entry_rent`]: sharing a helper with the implementation would let a
    /// wrong denominator or a missing entry write cancel out in the test.
    fn protocol_rent(bytes: u64, ledgers: u32, denominator: i64) -> i128 {
        let params = RentParameters::default();
        let rent = ceil_div(
            i128::from(bytes) * i128::from(params.fee_per_rent_1kb) * i128::from(ledgers),
            1024 * i128::from(denominator),
        );
        let ttl_bytes = ceil_div(
            i128::from(soroban_env_host::fees::TTL_ENTRY_SIZE)
                * i128::from(params.fee_per_write_1kb),
            1024,
        );
        rent + i128::from(params.fee_per_write_entry) + ttl_bytes
    }

    /// The rent term alone: what one more ledger of a horizon costs once the
    /// entry write and `TtlEntry` have been paid for.
    fn rent_only(bytes: u64, ledgers: u32, denominator: i64) -> i128 {
        let params = RentParameters::default();
        ceil_div(
            i128::from(bytes) * i128::from(params.fee_per_rent_1kb) * i128::from(ledgers),
            1024 * i128::from(denominator),
        )
    }

    #[test]
    fn a_footprint_without_entries_rents_as_nothing() {
        let snapshot = FootprintSnapshot::default();

        assert_eq!(snapshot.projected_rent(518_400), 0);
        assert_eq!(
            snapshot
                .rent_estimate(&RentParameters::default(), 1)
                .ledgers,
            1
        );
    }

    #[test]
    fn each_persistent_entry_pays_its_own_entry_write() {
        let estimate = priced().rent_estimate(&RentParameters::default(), 518_400);

        // `count` and `blob` are two ledger keys, so the network charges two entry
        // writes and two `TtlEntry` writes on top of the two rent terms.
        let expected =
            u64::try_from(protocol_rent(100, 518_400, 1_215) + protocol_rent(300, 518_400, 1_215))
                .unwrap();
        assert_eq!(estimate.persistent_stroops, expected);
    }

    #[test]
    fn instance_storage_rents_once_for_the_entry_that_holds_the_whole_map() {
        let estimate = priced().rent_estimate(&RentParameters::default(), 518_400);

        // `admin` and `config` are keys of one `ContractInstance`, not two ledger
        // entries: the tier is priced as one persistent entry of 40 + 60 bytes.
        let once = u64::try_from(protocol_rent(100, 518_400, 1_215)).unwrap();
        let per_key =
            u64::try_from(protocol_rent(40, 518_400, 1_215) + protocol_rent(60, 518_400, 1_215))
                .unwrap();
        assert_eq!(estimate.instance_stroops, once);
        assert!(
            estimate.instance_stroops < per_key,
            "pricing the instance map per key double-charges the entry write"
        );
    }

    #[test]
    fn temporary_entries_are_rented_more_cheaply_than_persistent_ones() {
        let params = RentParameters::default();
        let snapshot = {
            let mut snapshot = FootprintSnapshot::default();
            snapshot.insert(StorageTier::Temporary, "nonce", 100);
            snapshot.insert(StorageTier::Persistent, "count", 100);
            snapshot
        };

        let estimate = snapshot.rent_estimate(&params, 518_400);

        assert!(
            estimate.temporary_stroops < estimate.persistent_stroops,
            "temporary {} should rent below persistent {}",
            estimate.temporary_stroops,
            estimate.persistent_stroops
        );
        assert_eq!(
            estimate.temporary_stroops,
            u64::try_from(protocol_rent(100, 518_400, 2_430)).unwrap()
        );
    }

    #[test]
    fn doubling_the_horizon_adds_the_rent_but_not_the_entry_write() {
        let params = RentParameters::default();
        let snapshot = priced();

        let a_day = snapshot.rent_estimate(&params, 17_280).persistent_stroops;
        let two_days = snapshot.rent_estimate(&params, 34_560).persistent_stroops;

        // Both entries were written once in either projection, so the difference is
        // exactly the second horizon's rent and nothing else.
        let expected =
            u64::try_from(rent_only(100, 17_280, 1_215) + rent_only(300, 17_280, 1_215)).unwrap();
        assert_eq!(two_days - a_day, expected);
        assert!(two_days < a_day * 2, "the fixed write fee must not double");
    }

    #[test]
    fn the_rent_estimate_totals_across_every_tier() {
        let estimate = priced().rent_estimate(&RentParameters::default(), 518_400);

        assert_eq!(
            estimate.total_stroops(),
            estimate.instance_stroops + estimate.persistent_stroops + estimate.temporary_stroops
        );
        assert!(estimate.total_stroops() > 0);
    }

    #[test]
    fn the_rent_write_fee_floors_at_the_protocol_minimum() {
        assert_eq!(RentParameters::default().fee_per_rent_1kb, 1_000);
        assert_eq!(
            RentParameters::default()
                .with_soroban_state_size(0)
                .fee_per_rent_1kb,
            1_000,
            "an empty Soroban state still pays the floor, not the negative low value"
        );
    }

    #[test]
    fn the_rent_write_fee_reaches_its_high_value_at_the_target_state_size() {
        let at_target = RentParameters::default()
            .with_soroban_state_size(3_000_000_000)
            .fee_per_rent_1kb;
        let past_target = RentParameters::default()
            .with_soroban_state_size(6_000_000_000)
            .fee_per_rent_1kb;

        assert_eq!(at_target, 10_000);
        assert!(past_target > at_target);
    }

    #[test]
    fn measure_captures_one_contract_without_an_artifact() {
        let env = Env::default();
        let id = env.register(Store, ());
        StoreClient::new(&env, &id).seed();

        let measured = StorageFootprint::measure(&env, &id);

        assert_eq!(measured, FootprintSnapshot::capture(&env, &id));
        assert_eq!(measured.len(), 3);
        // One entry per tier, each carrying its own entry write.
        assert_eq!(
            u64::try_from(
                protocol_rent(measured.tier_bytes(StorageTier::Instance), 518_400, 1_215)
                    + protocol_rent(measured.get("persistent:count").unwrap(), 518_400, 1_215)
                    + protocol_rent(measured.get("temporary:nonce").unwrap(), 518_400, 2_430)
            )
            .unwrap(),
            measured.projected_rent(518_400)
        );
    }

    #[test]
    fn the_summary_table_lists_every_entry_and_subtotals_each_tier() {
        let markdown = priced().to_markdown();

        assert!(markdown.contains("| Key | Bytes |"));
        for key in [
            "instance:admin",
            "instance:config",
            "persistent:count",
            "persistent:blob",
            "temporary:nonce",
        ] {
            assert!(
                markdown.contains(&format!("| `{key}` |")),
                "missing a row for {key}:\n{markdown}"
            );
        }
        let estimate =
            priced().rent_estimate(&RentParameters::default(), DEFAULT_PROJECTION_LEDGERS);
        assert!(markdown.contains(&format!(
            "| **Total** | **5** | **580** | **{}** |",
            estimate.total_stroops()
        )));
        assert!(markdown
            .lines()
            .any(|line| line.starts_with("| persistent |") && line.ends_with('|')));
    }

    #[test]
    fn the_summary_table_of_an_empty_contract_still_has_a_row() {
        let markdown = FootprintSnapshot::default().to_markdown();

        assert!(markdown.contains("| _none_ | 0 |"));
        assert!(markdown.contains("| **Total** | **0** | **0** | **0** |"));
    }

    #[test]
    fn a_footprint_renders_one_section_per_contract_and_sums_its_rent() {
        let one = priced();
        let total = one.projected_rent(518_400);
        let mut footprint = StorageFootprint::new();
        footprint.record("store", one.clone());
        footprint.record("mirror", one);

        assert_eq!(footprint.projected_rent(518_400), total * 2);
        let markdown = footprint.to_markdown();
        assert!(markdown.contains("## `store`"));
        assert!(markdown.contains("## `mirror`"));
        assert_eq!(markdown.matches("### Entries").count(), 2);
    }
}
