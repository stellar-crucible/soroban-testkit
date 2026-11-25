//! Property-testing strategies for Soroban host cost models and fee schedules.
//!
//! Soroban meters execution across granular [`ContractCostType`] categories covering
//! WASM instruction execution, host memory allocation and copying, cryptographic
//! primitives, serialization, and VM lifecycle operations. Simulating resource
//! cost fluctuations requires generating realistic host cost models.
//!
//! The strategies in this module generate valid host cost schedules compatible with
//! `soroban-env-host`, support configuring custom fee multipliers across CPU and
//! Memory dimensions, and shrink towards baseline protocol fee schedules so that
//! failing cases simplify to the standard on-chain network costs.

use proptest::prelude::*;
use soroban_env_host::budget::Budget;
use soroban_env_host::storage::Storage;
use soroban_env_host::xdr::{
    ContractCostParamEntry, ContractCostParams, ContractCostType, ExtensionPoint,
};
use soroban_env_host::{Host, HostError};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// Resource dimension tracked by Soroban's metering subsystem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CostDimension {
    /// Host CPU instruction execution count.
    Cpu,
    /// Host memory bytes allocated or retained.
    Memory,
}

/// A linear cost component representing `const_term + (linear_term * input >> 7)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContractCostComponent {
    /// Constant overhead term.
    pub const_term: u64,
    /// Linear scaling term (scaled by 128 / 7 bits for fractional precision).
    pub linear_term: u64,
}

impl ContractCostComponent {
    /// Construct a new cost component with explicit constant and linear terms.
    pub const fn new(const_term: u64, linear_term: u64) -> Self {
        Self {
            const_term,
            linear_term,
        }
    }

    /// Evaluates the cost for a given iteration count and optional input size.
    pub fn evaluate(&self, iterations: u64, input: Option<u64>) -> u64 {
        let const_cost = self.const_term.saturating_mul(iterations);
        match input {
            Some(inp) => {
                let lin_cost = (self.linear_term as u128)
                    .saturating_mul(inp as u128)
                    .saturating_mul(iterations as u128)
                    >> 7;
                const_cost.saturating_add(lin_cost as u64)
            }
            None => const_cost,
        }
    }

    /// Converts this cost component into an XDR [`ContractCostParamEntry`].
    pub fn to_xdr_entry(&self) -> ContractCostParamEntry {
        ContractCostParamEntry {
            ext: ExtensionPoint::V0,
            const_term: (self.const_term as i64).max(0),
            linear_term: (self.linear_term as i64).max(0),
        }
    }
}

/// A cost entry for a single [`ContractCostType`], pairing CPU and Memory components.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContractCostEntry {
    /// The contract cost type discriminant.
    pub cost_type: ContractCostType,
    /// Cost component for host CPU instructions.
    pub cpu: ContractCostComponent,
    /// Cost component for host memory bytes.
    pub mem: ContractCostComponent,
}

impl ContractCostEntry {
    /// Construct a new cost entry.
    pub const fn new(
        cost_type: ContractCostType,
        cpu: ContractCostComponent,
        mem: ContractCostComponent,
    ) -> Self {
        Self {
            cost_type,
            cpu,
            mem,
        }
    }
}

/// Calibrated baseline protocol fee schedule values from `soroban-env-host`.
///
/// Format: `(ContractCostType, cpu_const, cpu_lin, mem_const, mem_lin)`.
pub const BASELINE_COST_ENTRIES: &[(ContractCostType, u64, u64, u64, u64)] = &[
    (ContractCostType::WasmInsnExec, 4, 0, 0, 0),
    (ContractCostType::MemAlloc, 434, 16, 16, 0),
    (ContractCostType::MemCpy, 42, 16, 0, 0),
    (ContractCostType::MemCmp, 44, 16, 0, 0),
    (ContractCostType::DispatchHostFunction, 295, 0, 0, 0),
    (ContractCostType::VisitObject, 60, 0, 0, 0),
    (ContractCostType::ValSer, 221, 26, 242, 0),
    (ContractCostType::ValDeser, 331, 4369, 0, 0),
    (ContractCostType::ComputeSha256Hash, 3636, 7013, 0, 0),
    (ContractCostType::ComputeEd25519PubKey, 40256, 0, 0, 0),
    (ContractCostType::VerifyEd25519Sig, 377551, 4059, 0, 0),
    (
        ContractCostType::VmInstantiation,
        417482,
        45712,
        132773,
        4903,
    ),
    (
        ContractCostType::VmCachedInstantiation,
        41142,
        634,
        69472,
        1217,
    ),
    (ContractCostType::InvokeVmFunction, 1945, 0, 14, 0),
    (ContractCostType::ComputeKeccak256Hash, 6481, 5943, 0, 0),
    (ContractCostType::DecodeEcdsaCurve256Sig, 711, 0, 0, 0),
    (
        ContractCostType::RecoverEcdsaSecp256k1Key,
        2314804,
        0,
        181,
        0,
    ),
    (ContractCostType::Int256AddSub, 4176, 0, 99, 0),
    (ContractCostType::Int256Mul, 4716, 0, 99, 0),
    (ContractCostType::Int256Div, 4680, 0, 99, 0),
    (ContractCostType::Int256Pow, 4256, 0, 99, 0),
    (ContractCostType::Int256Shift, 884, 0, 99, 0),
    (ContractCostType::ChaCha20DrawBytes, 1059, 502, 0, 0),
    (
        ContractCostType::ParseWasmInstructions,
        73077,
        25410,
        17564,
        6457,
    ),
    (ContractCostType::ParseWasmFunctions, 0, 540752, 0, 47464),
    (ContractCostType::ParseWasmGlobals, 0, 176363, 0, 13420),
    (ContractCostType::ParseWasmTableEntries, 0, 29989, 0, 6285),
    (ContractCostType::ParseWasmTypes, 0, 1061449, 0, 64670),
    (ContractCostType::ParseWasmDataSegments, 0, 237336, 0, 29074),
    (ContractCostType::ParseWasmElemSegments, 0, 328476, 0, 48095),
    (ContractCostType::ParseWasmImports, 0, 701845, 0, 103229),
    (ContractCostType::ParseWasmExports, 0, 429383, 0, 36394),
    (ContractCostType::ParseWasmDataSegmentBytes, 0, 28, 0, 257),
    (
        ContractCostType::InstantiateWasmInstructions,
        43030,
        0,
        70704,
        0,
    ),
    (
        ContractCostType::InstantiateWasmFunctions,
        0,
        7556,
        0,
        14613,
    ),
    (ContractCostType::InstantiateWasmGlobals, 0, 10711, 0, 6833),
    (
        ContractCostType::InstantiateWasmTableEntries,
        0,
        3300,
        0,
        1025,
    ),
    (ContractCostType::InstantiateWasmTypes, 0, 0, 0, 0),
    (
        ContractCostType::InstantiateWasmDataSegments,
        0,
        23038,
        0,
        129632,
    ),
    (
        ContractCostType::InstantiateWasmElemSegments,
        0,
        42488,
        0,
        13665,
    ),
    (
        ContractCostType::InstantiateWasmImports,
        0,
        828974,
        0,
        97637,
    ),
    (ContractCostType::InstantiateWasmExports, 0, 297100, 0, 9176),
    (
        ContractCostType::InstantiateWasmDataSegmentBytes,
        0,
        14,
        0,
        126,
    ),
    (ContractCostType::Sec1DecodePointUncompressed, 1882, 0, 0, 0),
    (ContractCostType::VerifyEcdsaSecp256r1Sig, 3000906, 0, 0, 0),
    (ContractCostType::Bls12381EncodeFp, 661, 0, 0, 0),
    (ContractCostType::Bls12381DecodeFp, 985, 0, 0, 0),
    (ContractCostType::Bls12381G1CheckPointOnCurve, 1934, 0, 0, 0),
    (
        ContractCostType::Bls12381G1CheckPointInSubgroup,
        730510,
        0,
        0,
        0,
    ),
    (ContractCostType::Bls12381G2CheckPointOnCurve, 5921, 0, 0, 0),
    (
        ContractCostType::Bls12381G2CheckPointInSubgroup,
        1057822,
        0,
        0,
        0,
    ),
    (
        ContractCostType::Bls12381G1ProjectiveToAffine,
        92642,
        0,
        0,
        0,
    ),
    (
        ContractCostType::Bls12381G2ProjectiveToAffine,
        100742,
        0,
        0,
        0,
    ),
    (ContractCostType::Bls12381G1Add, 7689, 0, 0, 0),
    (ContractCostType::Bls12381G1Mul, 2458985, 0, 0, 0),
    (
        ContractCostType::Bls12381G1Msm,
        2347584,
        94135478,
        109494,
        266603,
    ),
    (ContractCostType::Bls12381MapFpToG1, 1020885, 0, 2776, 0),
    (ContractCostType::Bls12381HashToG1, 2638451, 6803, 5896, 0),
    (ContractCostType::Bls12381G2Add, 25207, 0, 0, 0),
    (ContractCostType::Bls12381G2Mul, 7873219, 0, 0, 0),
    (
        ContractCostType::Bls12381G2Msm,
        7663880,
        298580871,
        219654,
        266603,
    ),
    (ContractCostType::Bls12381MapFp2ToG2, 1856539, 0, 1672, 0),
    (ContractCostType::Bls12381HashToG2, 6315452, 7232, 3960, 0),
    (
        ContractCostType::Bls12381Pairing,
        10558948,
        632860943,
        2204,
        9340474,
    ),
    (ContractCostType::Bls12381FrFromU256, 1994, 0, 0, 0),
    (ContractCostType::Bls12381FrToU256, 1155, 0, 248, 0),
    (ContractCostType::Bls12381FrAddSub, 74, 0, 0, 0),
    (ContractCostType::Bls12381FrMul, 332, 0, 0, 0),
    (ContractCostType::Bls12381FrPow, 691, 74558, 0, 128),
    (ContractCostType::Bls12381FrInv, 35421, 0, 0, 0),
    (ContractCostType::Bn254EncodeFp, 344, 0, 0, 0),
    (ContractCostType::Bn254DecodeFp, 476, 0, 0, 0),
    (ContractCostType::Bn254G1CheckPointOnCurve, 904, 0, 0, 0),
    (ContractCostType::Bn254G2CheckPointOnCurve, 2811, 0, 0, 0),
    (
        ContractCostType::Bn254G2CheckPointInSubgroup,
        1706052,
        0,
        0,
        0,
    ),
    (ContractCostType::Bn254G1ProjectiveToAffine, 61, 0, 0, 0),
    (ContractCostType::Bn254G1Add, 3623, 0, 0, 0),
    (ContractCostType::Bn254G1Mul, 1150435, 0, 0, 0),
    (
        ContractCostType::Bn254Pairing,
        5263916,
        392472814,
        1821,
        6232546,
    ),
    (ContractCostType::Bn254FrFromU256, 2052, 0, 0, 0),
    (ContractCostType::Bn254FrToU256, 1133, 0, 312, 0),
    (ContractCostType::Bn254FrAddSub, 74, 0, 0, 0),
    (ContractCostType::Bn254FrMul, 332, 0, 0, 0),
    (ContractCostType::Bn254FrPow, 755, 68930, 0, 0),
    (ContractCostType::Bn254FrInv, 33151, 0, 0, 0),
    (
        ContractCostType::Bn254G1Msm,
        1185193,
        41568084,
        73061,
        229779,
    ),
];

/// A generated realistic host cost model covering all standard [`ContractCostType`] enumerations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractCostModel {
    /// Cost entries keyed by position / cost type.
    entries: Vec<ContractCostEntry>,
    /// Global CPU multiplier in basis points (10,000 = 1.0x baseline).
    cpu_multiplier_bps: u32,
    /// Global Memory multiplier in basis points (10,000 = 1.0x baseline).
    mem_multiplier_bps: u32,
    /// Custom per-cost-type multiplier overrides in basis points.
    custom_multipliers: BTreeMap<(ContractCostType, CostDimension), u32>,
}

impl Default for ContractCostModel {
    fn default() -> Self {
        Self::baseline()
    }
}

impl ContractCostModel {
    /// Construct the baseline protocol fee schedule (1.0x multipliers, no modifications).
    pub fn baseline() -> Self {
        let entries = BASELINE_COST_ENTRIES
            .iter()
            .map(|&(ct, cc, cl, mc, ml)| {
                ContractCostEntry::new(
                    ct,
                    ContractCostComponent::new(cc, cl),
                    ContractCostComponent::new(mc, ml),
                )
            })
            .collect();

        Self {
            entries,
            cpu_multiplier_bps: 10_000,
            mem_multiplier_bps: 10_000,
            custom_multipliers: BTreeMap::new(),
        }
    }

    /// Construct a custom cost model from entries and multipliers.
    pub fn new(
        entries: Vec<ContractCostEntry>,
        cpu_multiplier_bps: u32,
        mem_multiplier_bps: u32,
        custom_multipliers: BTreeMap<(ContractCostType, CostDimension), u32>,
    ) -> Self {
        Self {
            entries,
            cpu_multiplier_bps,
            mem_multiplier_bps,
            custom_multipliers,
        }
    }

    /// Global CPU fee multiplier as a float (e.g. `1.0` for baseline, `2.5` for 250%).
    pub fn cpu_multiplier(&self) -> f64 {
        self.cpu_multiplier_bps as f64 / 10_000.0
    }

    /// Global Memory fee multiplier as a float (e.g. `1.0` for baseline).
    pub fn mem_multiplier(&self) -> f64 {
        self.mem_multiplier_bps as f64 / 10_000.0
    }

    /// Global CPU fee multiplier in basis points (10,000 = 100% / 1.0x).
    pub fn cpu_multiplier_bps(&self) -> u32 {
        self.cpu_multiplier_bps
    }

    /// Global Memory fee multiplier in basis points (10,000 = 100% / 1.0x).
    pub fn mem_multiplier_bps(&self) -> u32 {
        self.mem_multiplier_bps
    }

    /// Effective fee multiplier for a specific cost type and dimension in basis points.
    pub fn multiplier_bps_for(&self, ty: ContractCostType, dim: CostDimension) -> u32 {
        if let Some(&bps) = self.custom_multipliers.get(&(ty, dim)) {
            bps
        } else {
            match dim {
                CostDimension::Cpu => self.cpu_multiplier_bps,
                CostDimension::Memory => self.mem_multiplier_bps,
            }
        }
    }

    /// Effective fee multiplier for a specific cost type and dimension as a float.
    pub fn multiplier_for(&self, ty: ContractCostType, dim: CostDimension) -> f64 {
        self.multiplier_bps_for(ty, dim) as f64 / 10_000.0
    }

    /// Whether this cost model matches the exact baseline protocol fee schedule.
    pub fn is_baseline(&self) -> bool {
        self.cpu_multiplier_bps == 10_000
            && self.mem_multiplier_bps == 10_000
            && self.custom_multipliers.is_empty()
    }

    /// All cost entries in this model.
    pub fn entries(&self) -> &[ContractCostEntry] {
        &self.entries
    }

    /// Returns the cost entry for a given [`ContractCostType`].
    pub fn cost_type_entry(&self, ty: ContractCostType) -> Option<&ContractCostEntry> {
        self.entries.iter().find(|e| e.cost_type == ty)
    }

    /// Returns the CPU cost component for a given [`ContractCostType`].
    pub fn cpu_entry(&self, ty: ContractCostType) -> Option<&ContractCostComponent> {
        self.cost_type_entry(ty).map(|e| &e.cpu)
    }

    /// Returns the Memory cost component for a given [`ContractCostType`].
    pub fn mem_entry(&self, ty: ContractCostType) -> Option<&ContractCostComponent> {
        self.cost_type_entry(ty).map(|e| &e.mem)
    }

    /// Evaluates estimated CPU and Memory costs for a cost type.
    pub fn evaluate_cost(
        &self,
        ty: ContractCostType,
        iterations: u64,
        input: Option<u64>,
    ) -> (u64, u64) {
        let cpu = self
            .cpu_entry(ty)
            .map(|c| c.evaluate(iterations, input))
            .unwrap_or(0);
        let mem = self
            .mem_entry(ty)
            .map(|c| c.evaluate(iterations, input))
            .unwrap_or(0);
        (cpu, mem)
    }

    /// Builds a valid XDR [`ContractCostParams`] for CPU instructions compatible with `soroban-env-host`.
    pub fn cpu_params(&self) -> ContractCostParams {
        let entries: Vec<ContractCostParamEntry> =
            self.entries.iter().map(|e| e.cpu.to_xdr_entry()).collect();
        ContractCostParams(entries.try_into().expect("valid cost params length"))
    }

    /// Builds a valid XDR [`ContractCostParams`] for Memory bytes compatible with `soroban-env-host`.
    pub fn mem_params(&self) -> ContractCostParams {
        let entries: Vec<ContractCostParamEntry> =
            self.entries.iter().map(|e| e.mem.to_xdr_entry()).collect();
        ContractCostParams(entries.try_into().expect("valid cost params length"))
    }

    /// Constructs a `soroban-env-host` [`Budget`] initialized with this cost model and limits.
    pub fn try_into_budget(&self, cpu_limit: u64, mem_limit: u64) -> Result<Budget, HostError> {
        Budget::try_from_configs(cpu_limit, mem_limit, self.cpu_params(), self.mem_params())
    }

    /// Constructs a `soroban-env-host` [`Host`] configured with this cost model's budget.
    pub fn try_into_host(&self, cpu_limit: u64, mem_limit: u64) -> Result<Host, HostError> {
        let budget = self.try_into_budget(cpu_limit, mem_limit)?;
        Ok(Host::with_storage_and_budget(Storage::default(), budget))
    }
}

/// Configuration builder for custom contract cost model fuzzing strategies.
#[derive(Clone, Debug)]
pub struct ContractCostModelConfig {
    /// Range of CPU multipliers in percentage (100 = 100% = 1.0x baseline).
    pub cpu_multiplier_pct_range: RangeInclusive<u32>,
    /// Range of Memory multipliers in percentage (100 = 100% = 1.0x baseline).
    pub mem_multiplier_pct_range: RangeInclusive<u32>,
    /// Custom per-cost-type multiplier ranges in percentage.
    pub custom_type_pct_ranges: Vec<(ContractCostType, CostDimension, RangeInclusive<u32>)>,
}

impl Default for ContractCostModelConfig {
    fn default() -> Self {
        Self {
            cpu_multiplier_pct_range: 100..=500,
            mem_multiplier_pct_range: 100..=500,
            custom_type_pct_ranges: Vec::new(),
        }
    }
}

impl ContractCostModelConfig {
    /// Create a new default cost model strategy configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the range of CPU fee multipliers in percentage (e.g. `100..=400` for 1x-4x).
    pub fn with_cpu_multiplier_range(mut self, range: RangeInclusive<u32>) -> Self {
        self.cpu_multiplier_pct_range = range;
        self
    }

    /// Configure the range of Memory fee multipliers in percentage (e.g. `100..=300` for 1x-3x).
    pub fn with_mem_multiplier_range(mut self, range: RangeInclusive<u32>) -> Self {
        self.mem_multiplier_pct_range = range;
        self
    }

    /// Configure fee multipliers for an individual [`CostDimension`].
    pub fn with_dimension_multiplier_range(
        self,
        dimension: CostDimension,
        range: RangeInclusive<u32>,
    ) -> Self {
        match dimension {
            CostDimension::Cpu => self.with_cpu_multiplier_range(range),
            CostDimension::Memory => self.with_mem_multiplier_range(range),
        }
    }

    /// Configure a custom multiplier range for an individual [`ContractCostType`] and dimension.
    pub fn with_cost_type_multiplier_range(
        mut self,
        ty: ContractCostType,
        dim: CostDimension,
        range: RangeInclusive<u32>,
    ) -> Self {
        self.custom_type_pct_ranges.push((ty, dim, range));
        self
    }

    /// Converts this configuration into a `proptest` strategy generating [`ContractCostModel`].
    pub fn strategy(&self) -> impl Strategy<Value = ContractCostModel> {
        let cpu_range = self.cpu_multiplier_pct_range.clone();
        let mem_range = self.mem_multiplier_pct_range.clone();
        let custom_specs = self.custom_type_pct_ranges.clone();

        // Shrinking towards baseline:
        // We generate signed offset from 100 (where 0 offset corresponds to 100% baseline).
        // Proptest's integer shrinking shrinks signed integers towards 0.
        let cpu_offset_strat = shrinkable_offset_strategy(&cpu_range);
        let mem_offset_strat = shrinkable_offset_strategy(&mem_range);

        let custom_strats: Vec<_> = custom_specs
            .iter()
            .map(|&(ct, dim, ref rng)| {
                let off_strat = shrinkable_offset_strategy(rng);
                off_strat.prop_map(move |off| (ct, dim, (100i32 + off).max(1) as u32 * 100))
            })
            .collect();

        (cpu_offset_strat, mem_offset_strat, custom_strats).prop_map(
            move |(cpu_off, mem_off, custom_entries)| {
                let cpu_pct = (100i32 + cpu_off).max(1) as u32;
                let mem_pct = (100i32 + mem_off).max(1) as u32;
                let cpu_bps = cpu_pct * 100;
                let mem_bps = mem_pct * 100;

                let mut custom_map = BTreeMap::new();
                for (ct, dim, bps) in custom_entries {
                    custom_map.insert((ct, dim), bps);
                }

                let entries = BASELINE_COST_ENTRIES
                    .iter()
                    .map(|&(ct, cc, cl, mc, ml)| {
                        let eff_cpu_bps = custom_map
                            .get(&(ct, CostDimension::Cpu))
                            .copied()
                            .unwrap_or(cpu_bps);
                        let eff_mem_bps = custom_map
                            .get(&(ct, CostDimension::Memory))
                            .copied()
                            .unwrap_or(mem_bps);

                        let scaled_cpu_const = scale_val(cc, eff_cpu_bps);
                        let scaled_cpu_lin = scale_val(cl, eff_cpu_bps);
                        let scaled_mem_const = scale_val(mc, eff_mem_bps);
                        let scaled_mem_lin = scale_val(ml, eff_mem_bps);

                        ContractCostEntry::new(
                            ct,
                            ContractCostComponent::new(scaled_cpu_const, scaled_cpu_lin),
                            ContractCostComponent::new(scaled_mem_const, scaled_mem_lin),
                        )
                    })
                    .collect();

                ContractCostModel::new(entries, cpu_bps, mem_bps, custom_map)
            },
        )
    }
}

/// Helper to produce a signed offset strategy from 100% that proptest shrinks towards 0.
fn shrinkable_offset_strategy(range: &RangeInclusive<u32>) -> impl Strategy<Value = i32> {
    let min = *range.start();
    let max = *range.end();
    let min_off = min as i32 - 100;
    let max_off = max as i32 - 100;
    min_off..=max_off
}

/// Scales a baseline value by basis points (10,000 = 1.0x), keeping non-zero values >= 1.
fn scale_val(val: u64, bps: u32) -> u64 {
    if val == 0 {
        return 0;
    }
    let scaled = (val as u128).saturating_mul(bps as u128) / 10_000;
    (scaled as u64).max(1)
}

/// Generates a realistic [`ContractCostModel`] covering all standard [`ContractCostType`] enumerations.
///
/// Fuzzes fee shifts between 100% (baseline) and 500% across CPU and Memory dimensions,
/// shrinking towards the baseline protocol fee schedule (100% multiplier, 0 offset).
pub fn contract_cost_model() -> impl Strategy<Value = ContractCostModel> {
    ContractCostModelConfig::default().strategy()
}

/// Generates a [`ContractCostModel`] with explicit custom multiplier ranges for CPU and Memory.
///
/// Both ranges are percentages (e.g. `100..=300` for 1x to 3x fees).
/// Shrinks towards the baseline protocol fee schedule (`100%`).
pub fn contract_cost_model_with_multipliers(
    cpu_range: RangeInclusive<u32>,
    mem_range: RangeInclusive<u32>,
) -> impl Strategy<Value = ContractCostModel> {
    ContractCostModelConfig::new()
        .with_cpu_multiplier_range(cpu_range)
        .with_mem_multiplier_range(mem_range)
        .strategy()
}

/// Generates a [`ContractCostModel`] using a builder [`ContractCostModelConfig`].
pub fn contract_cost_model_with_config(
    config: ContractCostModelConfig,
) -> impl Strategy<Value = ContractCostModel> {
    config.strategy()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_env_host::xdr::ContractCostType;

    #[test]
    fn test_baseline_cost_model_matches_protocol_defaults() {
        let model = ContractCostModel::baseline();
        assert!(model.is_baseline());
        assert_eq!(model.cpu_multiplier_bps(), 10_000);
        assert_eq!(model.mem_multiplier_bps(), 10_000);
        assert_eq!(model.entries().len(), BASELINE_COST_ENTRIES.len());
        assert_eq!(model.entries().len(), ContractCostType::variants().len());

        let wasm_entry = model
            .cost_type_entry(ContractCostType::WasmInsnExec)
            .unwrap();
        assert_eq!(wasm_entry.cpu.const_term, 4);
        assert_eq!(wasm_entry.cpu.linear_term, 0);

        let mem_alloc = model.cost_type_entry(ContractCostType::MemAlloc).unwrap();
        assert_eq!(mem_alloc.cpu.const_term, 434);
        assert_eq!(mem_alloc.cpu.linear_term, 16);
        assert_eq!(mem_alloc.mem.const_term, 16);

        // Verify budget creation from baseline
        let budget = model.try_into_budget(1_000_000, 1_000_000).unwrap();
        assert_eq!(budget.get_cpu_insns_consumed().unwrap(), 0);
        assert_eq!(budget.get_mem_bytes_consumed().unwrap(), 0);

        // Charge some costs
        budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
        assert_eq!(budget.get_cpu_insns_consumed().unwrap(), 4);

        budget
            .charge(ContractCostType::MemAlloc, Some(1024))
            .unwrap();
        assert!(budget.get_cpu_insns_consumed().unwrap() > 434);
    }

    #[test]
    fn test_try_into_host_and_custom_budget() {
        let model = ContractCostModel::baseline();
        let host = model.try_into_host(500_000, 500_000).unwrap();
        host.charge_budget(ContractCostType::WasmInsnExec, None)
            .unwrap();
        host.with_budget(|budget| {
            assert_eq!(budget.get_cpu_insns_consumed().unwrap(), 4);
            Ok(())
        })
        .unwrap();
    }

    proptest! {
        #[test]
        fn test_contract_cost_model_generates_valid_host_schedules(
            model in contract_cost_model()
        ) {
            // Must cover all variants
            assert_eq!(model.entries().len(), ContractCostType::variants().len());

            // Must produce valid ContractCostParams that soroban-env-host accepts
            let cpu_params = model.cpu_params();
            let mem_params = model.mem_params();
            assert_eq!(cpu_params.0.len(), ContractCostType::variants().len());
            assert_eq!(mem_params.0.len(), ContractCostType::variants().len());

            // Must successfully construct a soroban-env-host Budget
            let budget = model.try_into_budget(10_000_000, 10_000_000)
                .expect("Valid host budget from generated cost model");

            // Must meter charges consistently
            budget.charge(ContractCostType::WasmInsnExec, None)
                .expect("WasmInsnExec charge succeeds");
            assert!(budget.get_cpu_insns_consumed().unwrap() >= 4);

            budget.charge(ContractCostType::MemAlloc, Some(256))
                .expect("MemAlloc charge succeeds");
            assert!(budget.get_mem_bytes_consumed().unwrap() >= 16);
        }

        #[test]
        fn test_fee_multiplier_scaling(
            model in contract_cost_model_with_multipliers(150..=300, 200..=400)
        ) {
            let cpu_mult = model.cpu_multiplier();
            let mem_mult = model.mem_multiplier();

            assert!((1.5..=3.0).contains(&cpu_mult));
            assert!((2.0..=4.0).contains(&mem_mult));

            // Verify WasmInsnExec const_term scaled by cpu_multiplier
            let wasm = model.cost_type_entry(ContractCostType::WasmInsnExec).unwrap();
            let expected_min_cpu = (4.0 * 1.5) as u64;
            assert!(wasm.cpu.const_term >= expected_min_cpu);

            // Verify MemAlloc const_term scaled by mem_multiplier
            let mem_alloc = model.cost_type_entry(ContractCostType::MemAlloc).unwrap();
            let expected_min_mem = (16.0 * 2.0) as u64;
            assert!(mem_alloc.mem.const_term >= expected_min_mem);
        }

        #[test]
        fn test_custom_cost_dimension_overrides(
            model in contract_cost_model_with_config(
                ContractCostModelConfig::new()
                    .with_cpu_multiplier_range(100..=120)
                    .with_mem_multiplier_range(100..=120)
                    .with_cost_type_multiplier_range(
                        ContractCostType::VerifyEd25519Sig,
                        CostDimension::Cpu,
                        500..=800,
                    )
            )
        ) {
            let ed_cpu_bps = model.multiplier_bps_for(
                ContractCostType::VerifyEd25519Sig,
                CostDimension::Cpu,
            );
            assert!((50_000..=80_000).contains(&ed_cpu_bps));

            let ed_entry = model.cost_type_entry(ContractCostType::VerifyEd25519Sig).unwrap();
            // Baseline const is 377,551; under 5x-8x it should be >= 1,887,755
            assert!(ed_entry.cpu.const_term >= 1_887_755);

            // Other entries should stay in the 100-120% range
            let wasm = model.cost_type_entry(ContractCostType::WasmInsnExec).unwrap();
            assert!(wasm.cpu.const_term <= 6);
        }

        #[test]
        fn test_budget_threshold_fuzzing(
            model in contract_cost_model_with_multipliers(100..=400, 100..=400)
        ) {
            // Suppose a contract execution consists of:
            // - 1,000 Wasm instructions
            // - 2 Memory allocations of 512 bytes each
            // - 1 SHA-256 hash of 64 bytes
            let (wasm_cpu, wasm_mem) = model.evaluate_cost(ContractCostType::WasmInsnExec, 1_000, None);
            let (alloc_cpu, alloc_mem) = model.evaluate_cost(ContractCostType::MemAlloc, 2, Some(512));
            let (sha_cpu, sha_mem) = model.evaluate_cost(ContractCostType::ComputeSha256Hash, 1, Some(64));

            let total_cpu = wasm_cpu + alloc_cpu + sha_cpu;
            let _total_mem = wasm_mem + alloc_mem + sha_mem;

            if model.is_baseline() {
                assert!(total_cpu <= 15_000);
            } else {
                // Under fee shifts, total_cpu scales proportionally
                assert!(total_cpu >= 12_000);
            }

            // Demonstrating budget threshold testing:
            // A contract with tight budget ceiling (e.g. 20,000 CPU) will fit under baseline
            // but exceed under a 2x-4x fee surge.
            let budget = model.try_into_budget(20_000, 50_000).unwrap();
            budget.charge(ContractCostType::WasmInsnExec, None).unwrap();
            assert!(budget.get_cpu_insns_remaining().unwrap() > 0);
        }

        #[test]
        fn test_proptest_contract_execution_budget_threshold_fuzzing(
            model in contract_cost_model_with_multipliers(100..=500, 100..=500),
            iterations in 100u64..=1_000u64,
            payload_size in 32u64..=256u64,
        ) {
            // Budget provisioned for baseline execution + 25% safety margin:
            let baseline = ContractCostModel::baseline();
            let (base_wasm, _) = baseline.evaluate_cost(ContractCostType::WasmInsnExec, iterations, None);
            let (base_hash, _) = baseline.evaluate_cost(ContractCostType::ComputeSha256Hash, 1, Some(payload_size));
            let (base_mem, _) = baseline.evaluate_cost(ContractCostType::MemAlloc, 1, Some(payload_size));
            let baseline_needed = base_wasm + base_hash + base_mem;
            let budget_threshold = (baseline_needed as f64 * 1.25) as u64;

            // Execute the workload against the fuzzed host budget schedule:
            let budget = model.try_into_budget(budget_threshold, 1_000_000).unwrap();

            // Charge the contract operations:
            let res = (|| -> Result<(), HostError> {
                budget.bulk_charge(ContractCostType::WasmInsnExec, iterations, None)?;
                budget.charge(ContractCostType::ComputeSha256Hash, Some(payload_size))?;
                budget.charge(ContractCostType::MemAlloc, Some(payload_size))?;
                Ok(())
            })();

            // If the model is at baseline (1.0x), it fits within the 1.25x threshold:
            if model.is_baseline() {
                assert!(res.is_ok(), "Baseline cost model must fit within budget threshold");
            } else if model.cpu_multiplier() >= 1.5 {
                // If fees shifted >= 1.5x, it exceeds the 1.25x threshold:
                assert!(res.is_err(), "Shifted fees >= 1.5x must exceed 1.25x threshold");
            }
        }
    }

    #[test]
    fn test_shrinking_towards_baseline() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;

        fn minimise<T: Strategy>(strategy: T, fails: impl Fn(&T::Value) -> bool) -> T::Value {
            let mut runner = TestRunner::deterministic();
            let mut tree = loop {
                let tree = strategy.new_tree(&mut runner).unwrap();
                if fails(&tree.current()) {
                    break tree;
                }
            };
            let mut smallest = tree.current();
            loop {
                if fails(&tree.current()) {
                    smallest = tree.current();
                    if !tree.simplify() {
                        break;
                    }
                } else if !tree.complicate() {
                    break;
                }
            }
            smallest
        }

        // Suppose a test fails whenever CPU multiplier is >= 150% (multiplier_bps >= 15_000)
        let strat = contract_cost_model_with_multipliers(100..=500, 100..=500);
        let shrunk = minimise(strat, |m| m.cpu_multiplier_bps() >= 15_000);

        // It should shrink towards the threshold boundary, and when failing everywhere (e.g. >= 100%),
        // it shrinks directly to the baseline 100% (10,000 bps)!
        assert!(shrunk.cpu_multiplier_bps() >= 15_000);

        // When failing for any model, it shrinks all the way to baseline (10,000 bps / 1.0x)
        let strat_all = contract_cost_model_with_multipliers(100..=500, 100..=500);
        let shrunk_to_baseline = minimise(strat_all, |_| true);
        assert_eq!(shrunk_to_baseline.cpu_multiplier_bps(), 10_000);
        assert_eq!(shrunk_to_baseline.mem_multiplier_bps(), 10_000);
        assert!(shrunk_to_baseline.is_baseline());
    }
}
