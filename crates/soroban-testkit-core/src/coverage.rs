//! Automated dead code and uninvoked contract function detector.
//!
//! Integration test suites can pass with 100% green checks while failing to invoke
//! critical administrative, recovery, or emergency functions defined in contract bytecode,
//! leaving potential vulnerabilities unexercised.
//!
//! [`CoverageAuditor`] extracts all public function entry points from contract WASM bytecode,
//! contract spec XDR, or client definitions, tracks invocations observed via Soroban host
//! diagnostic events across test executions, and produces comprehensive coverage reports
//! in both Markdown and text formats.
//!
//! # Quick Start
//!
//! ```rust
//! use soroban_testkit_core::{CoverageAuditor, client_definition};
//!
//! let mut auditor = CoverageAuditor::new();
//! let def = client_definition!("Vault", [init, deposit, withdraw, emergency_stop]);
//! auditor.register_client_definition("CAAA111", &def);
//! auditor.ignore_function("CAAA111", "init");
//!
//! auditor.record_invocation("CAAA111", "deposit");
//!
//! let report = auditor.report();
//! println!("{}", report.to_markdown());
//! assert_eq!(
//!     report.uninvoked_entry_points(),
//!     vec![("Vault", "emergency_stop"), ("Vault", "withdraw")]
//! );
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Cursor;

use soroban_env_host::events::HostEvent;
use soroban_env_host::xdr::{
    ContractEventBody, ContractEventType, Hash, Limited, Limits, ReadXdr, ScAddress, ScVal,
};
use soroban_sdk::xdr::{ContractId, ScSpecEntry};
use soroban_sdk::{Address, Env};

use crate::stack_depth::contract_strkey;

/// Errors that may occur when parsing contract WASM bytecode or spec XDR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverageError {
    /// The supplied WASM bytecode is malformed, truncated, or lacks required headers.
    InvalidWasm(&'static str),
    /// An error occurred while decoding contract specification XDR data.
    XdrDecodeError(String),
    /// Contract was not found in the auditor registry.
    ContractNotFound(String),
}

impl fmt::Display for CoverageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoverageError::InvalidWasm(msg) => write!(f, "Invalid contract WASM: {msg}"),
            CoverageError::XdrDecodeError(msg) => write!(f, "XDR decode error: {msg}"),
            CoverageError::ContractNotFound(key) => {
                write!(f, "Contract '{key}' not registered in CoverageAuditor")
            }
        }
    }
}

impl std::error::Error for CoverageError {}

/// Trait for converting various contract identifier representations into a canonical string key.
pub trait AsContractKey {
    /// Return the canonical contract key string.
    fn to_contract_key(&self) -> String;
}

impl AsContractKey for &str {
    fn to_contract_key(&self) -> String {
        self.to_string()
    }
}

impl AsContractKey for String {
    fn to_contract_key(&self) -> String {
        self.clone()
    }
}

impl AsContractKey for &String {
    fn to_contract_key(&self) -> String {
        (*self).clone()
    }
}

impl AsContractKey for Address {
    fn to_contract_key(&self) -> String {
        extract_address_strkey(self)
    }
}

impl AsContractKey for &Address {
    fn to_contract_key(&self) -> String {
        extract_address_strkey(self)
    }
}

/// Extract the `C...` strkey or canonical representation from an SDK [`Address`].
pub fn extract_address_strkey(address: &Address) -> String {
    let debug = format!("{address:?}");
    if let Some(rest) = debug.strip_prefix("Contract(") {
        if let Some(inner) = rest.strip_suffix(')') {
            return inner.to_string();
        }
    }
    if let Some(rest) = debug.strip_prefix("Account(") {
        if let Some(inner) = rest.strip_suffix(')') {
            return inner.to_string();
        }
    }
    debug
}

/// Representation of a contract's public client interface definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientDefinition {
    /// Logical name of the contract (e.g. `"DynamicAMM"` or `"Vault"`).
    pub contract_name: String,
    /// Public entry points exposed by this contract client.
    pub functions: Vec<String>,
}

impl ClientDefinition {
    /// Create a new client definition for the specified contract name.
    pub fn new(contract_name: impl Into<String>) -> Self {
        Self {
            contract_name: contract_name.into(),
            functions: Vec::new(),
        }
    }

    /// Add a public function to the client definition.
    pub fn function(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        if !self.functions.contains(&name) {
            self.functions.push(name);
        }
        self
    }

    /// Add multiple public functions to the client definition.
    pub fn functions(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        for name in names {
            let name = name.into();
            if !self.functions.contains(&name) {
                self.functions.push(name);
            }
        }
        self
    }

    /// Create a client definition with an initial set of functions.
    pub fn with_functions(
        contract_name: impl Into<String>,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self::new(contract_name).functions(names)
    }

    /// Get the slice of defined public functions.
    pub fn public_functions(&self) -> &[String] {
        &self.functions
    }
}

/// Macro for cleanly defining a [`ClientDefinition`].
///
/// ```rust
/// use soroban_testkit_core::client_definition;
///
/// let def = client_definition!("TokenContract", [initialize, transfer, balance, burn]);
/// assert_eq!(def.contract_name, "TokenContract");
/// assert_eq!(def.public_functions(), &["initialize", "transfer", "balance", "burn"]);
/// ```
#[macro_export]
macro_rules! client_definition {
    ($name:expr, [ $($fn_name:ident),* $(,)? ]) => {
        $crate::coverage::ClientDefinition::new($name)
            $(.function(stringify!($fn_name)))*
    };
}

/// Status of a contract entry point during a coverage audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FunctionCoverageStatus {
    /// Function was invoked at least once during the audit.
    Invoked,
    /// Function was never invoked during the audit (dead code or unexercised entry point).
    Uninvoked,
    /// Function was intentionally ignored (e.g. initialization or one-time setup).
    Ignored,
}

impl FunctionCoverageStatus {
    /// String identifier for this status.
    pub fn as_str(&self) -> &'static str {
        match self {
            FunctionCoverageStatus::Invoked => "INVOKED",
            FunctionCoverageStatus::Uninvoked => "UNINVOKED",
            FunctionCoverageStatus::Ignored => "IGNORED",
        }
    }
}

impl fmt::Display for FunctionCoverageStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Detailed coverage metrics for a single contract function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCoverageRecord {
    /// Function entry point name.
    pub name: String,
    /// Number of times this function was invoked during the audit.
    pub invocations: usize,
    /// Coverage status (Invoked, Uninvoked, or Ignored).
    pub status: FunctionCoverageStatus,
}

/// Coverage summary for a single monitored contract.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractCoverageReport {
    /// Canonical contract identifier (`C...` strkey or alias).
    pub contract_id: String,
    /// Optional human-readable contract name.
    pub contract_name: Option<String>,
    /// Total number of public functions known.
    pub total_functions: usize,
    /// Number of functions invoked at least once.
    pub invoked_count: usize,
    /// Number of functions that received zero invocations.
    pub uninvoked_count: usize,
    /// Number of functions explicitly excluded via ignore lists.
    pub ignored_count: usize,
    /// Coverage percentage (invoked / (total - ignored) * 100.0).
    pub coverage_percent: f64,
    /// Per-function coverage records.
    pub functions: Vec<FunctionCoverageRecord>,
}

impl ContractCoverageReport {
    /// True when all actionable (non-ignored) functions were executed.
    pub fn is_fully_covered(&self) -> bool {
        self.uninvoked_count == 0
    }

    /// Return the list of uninvoked function names.
    pub fn uninvoked_functions(&self) -> Vec<&str> {
        self.functions
            .iter()
            .filter(|f| f.status == FunctionCoverageStatus::Uninvoked)
            .map(|f| f.name.as_str())
            .collect()
    }

    /// Return the list of invoked function names.
    pub fn invoked_functions(&self) -> Vec<&str> {
        self.functions
            .iter()
            .filter(|f| f.status == FunctionCoverageStatus::Invoked)
            .map(|f| f.name.as_str())
            .collect()
    }

    /// Return the list of ignored function names.
    pub fn ignored_functions(&self) -> Vec<&str> {
        self.functions
            .iter()
            .filter(|f| f.status == FunctionCoverageStatus::Ignored)
            .map(|f| f.name.as_str())
            .collect()
    }

    /// Look up the coverage record for a specific function.
    pub fn function_coverage(&self, name: &str) -> Option<&FunctionCoverageRecord> {
        self.functions.iter().find(|f| f.name == name)
    }

    /// Display name of the contract (either the given name or strkey).
    pub fn display_name(&self) -> &str {
        self.contract_name.as_deref().unwrap_or(&self.contract_id)
    }
}

/// Aggregate coverage report across all monitored contracts in a test run.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverageReport {
    /// Number of contracts evaluated.
    pub total_contracts: usize,
    /// Total public functions across all contracts.
    pub total_functions: usize,
    /// Total functions invoked.
    pub total_invoked: usize,
    /// Total functions that received zero invocations.
    pub total_uninvoked: usize,
    /// Total functions ignored.
    pub total_ignored: usize,
    /// Aggregate coverage percentage across all actionable functions.
    pub overall_coverage_percent: f64,
    /// Breakdown per contract, sorted by contract id.
    pub contracts: BTreeMap<String, ContractCoverageReport>,
}

impl CoverageReport {
    /// True when all actionable functions across all monitored contracts were executed.
    pub fn is_fully_covered(&self) -> bool {
        self.total_uninvoked == 0
    }

    /// Return coverage report for a specific contract.
    pub fn contract(&self, key: impl AsContractKey) -> Option<&ContractCoverageReport> {
        let key_str = key.to_contract_key();
        self.contracts.get(&key_str).or_else(|| {
            self.contracts
                .values()
                .find(|c| c.contract_name.as_deref() == Some(&key_str))
        })
    }

    /// List all uninvoked entry points as pairs of `(contract_identifier, function_name)`.
    pub fn uninvoked_entry_points(&self) -> Vec<(&str, &str)> {
        let mut list = Vec::new();
        for report in self.contracts.values() {
            let label = report.display_name();
            for func in report.uninvoked_functions() {
                list.push((label, func));
            }
        }
        list
    }

    /// Format this report as a Markdown document with summary tables, per-contract breakdowns, and alerts.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Contract Function Coverage Report\n\n");

        md.push_str("| Contract | Total Functions | Invoked | Uninvoked | Ignored | Coverage |\n");
        md.push_str("| :--- | :---: | :---: | :---: | :---: | :---: |\n");
        for report in self.contracts.values() {
            let label = match &report.contract_name {
                Some(name) => format!("`{name}` (`{}`)", report.contract_id),
                None => format!("`{}`", report.contract_id),
            };
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {:.1}% |\n",
                label,
                report.total_functions,
                report.invoked_count,
                report.uninvoked_count,
                report.ignored_count,
                report.coverage_percent,
            ));
        }
        md.push_str(&format!(
            "| **Total** | **{}** | **{}** | **{}** | **{}** | **{:.1}%** |\n\n",
            self.total_functions,
            self.total_invoked,
            self.total_uninvoked,
            self.total_ignored,
            self.overall_coverage_percent,
        ));

        for report in self.contracts.values() {
            let header_name = match &report.contract_name {
                Some(name) => format!("`{name}` (`{}`)", report.contract_id),
                None => format!("`{}`", report.contract_id),
            };
            md.push_str(&format!("## Contract: {header_name}\n\n"));
            md.push_str("| Function | Status | Invocations | Notes |\n");
            md.push_str("| :--- | :--- | :---: | :--- |\n");
            for func in &report.functions {
                let (status_str, notes) = match func.status {
                    FunctionCoverageStatus::Invoked => ("✓ Invoked", "-"),
                    FunctionCoverageStatus::Uninvoked => (
                        "✗ **Uninvoked**",
                        "**Dead code or unexercised entry point**",
                    ),
                    FunctionCoverageStatus::Ignored => {
                        ("⊘ Ignored", "Configured ignore list (setup/initialization)")
                    }
                };
                md.push_str(&format!(
                    "| `{}` | {} | {} | {} |\n",
                    func.name, status_str, func.invocations, notes
                ));
            }
            md.push('\n');
        }

        if self.total_uninvoked > 0 {
            md.push_str(&format!(
                "> [!WARNING]\n> **{} uninvoked contract entry point(s) detected.** Unexercised administrative, emergency, or recovery functions leave critical code paths untested.\n",
                self.total_uninvoked
            ));
        } else {
            md.push_str("> [!NOTE]\n> **Full contract function coverage achieved (100.0%).** All actionable public functions were executed during testing.\n");
        }

        md
    }

    /// Format this report as a plaintext summary table.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        text.push_str(
            "================================================================================\n",
        );
        text.push_str("CONTRACT FUNCTION COVERAGE REPORT\n");
        text.push_str(
            "================================================================================\n",
        );
        text.push_str(&format!(
            "Overall Coverage: {:.1}% ({}/{} actionable functions invoked, {} uninvoked, {} ignored)\n",
            self.overall_coverage_percent,
            self.total_invoked,
            self.total_functions.saturating_sub(self.total_ignored),
            self.total_uninvoked,
            self.total_ignored,
        ));
        text.push_str(&format!(
            "Contracts Monitored: {}\n\n",
            self.total_contracts
        ));

        for report in self.contracts.values() {
            let header = match &report.contract_name {
                Some(name) => format!("{name} ({})", report.contract_id),
                None => report.contract_id.clone(),
            };
            text.push_str(&format!(
                "Contract: {} - Coverage: {:.1}%\n",
                header, report.coverage_percent
            ));
            for func in &report.functions {
                match func.status {
                    FunctionCoverageStatus::Invoked => {
                        text.push_str(&format!(
                            "  [x] {:<24} - {} call(s)\n",
                            func.name, func.invocations
                        ));
                    }
                    FunctionCoverageStatus::Uninvoked => {
                        text.push_str(&format!(
                            "  [ ] {:<24} - 0 call(s) [UNINVOKED]\n",
                            func.name
                        ));
                    }
                    FunctionCoverageStatus::Ignored => {
                        text.push_str(&format!(
                            "  [-] {:<24} - {} call(s) [IGNORED]\n",
                            func.name, func.invocations
                        ));
                    }
                }
            }
            text.push('\n');
        }

        text.push_str(
            "--------------------------------------------------------------------------------\n",
        );
        if self.total_uninvoked > 0 {
            text.push_str(&format!(
                "WARNING: {} uninvoked contract function(s) detected:\n",
                self.total_uninvoked
            ));
            for (contract, func) in self.uninvoked_entry_points() {
                text.push_str(&format!("  ! {}::{}\n", contract, func));
            }
            text.push_str("STATUS: INCOMPLETE (unexercised entry points present)\n");
        } else {
            text.push_str("STATUS: COMPLETE (100% actionable coverage)\n");
        }
        text.push_str(
            "================================================================================\n",
        );
        text
    }

    /// Panic with the full report if any non-ignored public contract functions were not invoked.
    pub fn assert_full_coverage(&self) {
        if !self.is_fully_covered() {
            let uninvoked = self.uninvoked_entry_points();
            panic!(
                "Contract function coverage assertion failed! {} uninvoked function(s):\n{:?}\n\n{}",
                self.total_uninvoked,
                uninvoked,
                self.to_text()
            );
        }
    }

    /// Panic if overall actionable coverage falls below `min_percent`.
    pub fn assert_min_coverage(&self, min_percent: f64) {
        if self.overall_coverage_percent < min_percent {
            panic!(
                "Contract function coverage below minimum threshold: {:.1}% < {:.1}%\n\n{}",
                self.overall_coverage_percent,
                min_percent,
                self.to_text()
            );
        }
    }
}

impl fmt::Display for CoverageReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_text())
    }
}

/// Internal state tracked for a single registered contract.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ContractState {
    contract_id: String,
    name: Option<String>,
    functions: BTreeSet<String>,
    ignored: BTreeSet<String>,
    invocations: BTreeMap<String, usize>,
}

/// Automated dead code and uninvoked contract function auditor.
///
/// Tracks public contract entry points across a test suite and reports any entry points
/// that were never invoked.
#[derive(Debug, Clone, Default)]
pub struct CoverageAuditor {
    contracts: BTreeMap<String, ContractState>,
    aliases: BTreeMap<String, String>,
    global_ignored: BTreeSet<String>,
    last_observed_events: BTreeMap<usize, Vec<HostEvent>>,
}

impl CoverageAuditor {
    /// Create a new, empty [`CoverageAuditor`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a contract by its identifier or address with a list of known public functions.
    pub fn register_contract(
        &mut self,
        contract: impl AsContractKey,
        functions: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        let key = contract.to_contract_key();
        let state = self
            .contracts
            .entry(key.clone())
            .or_insert_with(|| ContractState {
                contract_id: key.clone(),
                name: None,
                functions: BTreeSet::new(),
                ignored: BTreeSet::new(),
                invocations: BTreeMap::new(),
            });
        for func in functions {
            state.functions.insert(func.into());
        }
        self
    }

    /// Register a contract with both a human-readable name and an identifier/address.
    pub fn register_contract_with_name(
        &mut self,
        name: impl Into<String>,
        contract: impl AsContractKey,
        functions: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        let name_str = name.into();
        let key = contract.to_contract_key();

        self.aliases.insert(name_str.clone(), key.clone());

        let state = self
            .contracts
            .entry(key.clone())
            .or_insert_with(|| ContractState {
                contract_id: key.clone(),
                name: Some(name_str.clone()),
                functions: BTreeSet::new(),
                ignored: BTreeSet::new(),
                invocations: BTreeMap::new(),
            });
        state.name = Some(name_str);
        for func in functions {
            state.functions.insert(func.into());
        }
        self
    }

    /// Register a contract by extracting its public functions directly from WASM bytecode.
    pub fn register_contract_wasm(
        &mut self,
        contract: impl AsContractKey,
        wasm: &[u8],
    ) -> Result<&mut Self, CoverageError> {
        let fns = Self::extract_wasm_functions(wasm)?;
        self.register_contract(contract, fns);
        Ok(self)
    }

    /// Register a contract with a human-readable name by extracting functions from WASM bytecode.
    pub fn register_contract_wasm_with_name(
        &mut self,
        name: impl Into<String>,
        contract: impl AsContractKey,
        wasm: &[u8],
    ) -> Result<&mut Self, CoverageError> {
        let fns = Self::extract_wasm_functions(wasm)?;
        self.register_contract_with_name(name, contract, fns);
        Ok(self)
    }

    /// Register a contract by extracting functions from serialized contract specification XDR bytes.
    pub fn register_contract_spec_xdr(
        &mut self,
        contract: impl AsContractKey,
        spec_xdr: &[u8],
    ) -> Result<&mut Self, CoverageError> {
        let fns = Self::extract_spec_xdr_functions(spec_xdr)?;
        self.register_contract(contract, fns);
        Ok(self)
    }

    /// Register a contract using a [`ClientDefinition`].
    pub fn register_client_definition(
        &mut self,
        contract: impl AsContractKey,
        client_def: &ClientDefinition,
    ) -> &mut Self {
        self.register_contract_with_name(&client_def.contract_name, contract, &client_def.functions)
    }

    /// Extract all public functions from contract WASM bytecode.
    ///
    /// Inspects the `contractspecv0` custom section if present; otherwise inspects
    /// the WebAssembly Export section to discover exported entry point functions.
    pub fn extract_wasm_functions(mut wasm: &[u8]) -> Result<Vec<String>, CoverageError> {
        if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
            return Err(CoverageError::InvalidWasm("invalid WASM magic header"));
        }
        // Skip magic (4) and version (4)
        wasm = &wasm[8..];

        let mut spec_functions = Vec::new();
        let mut exported_functions = Vec::new();

        while !wasm.is_empty() {
            let section_id = wasm[0];
            wasm = &wasm[1..];
            let section_len = read_u32_leb128(&mut wasm)? as usize;
            if wasm.len() < section_len {
                return Err(CoverageError::InvalidWasm("truncated WASM section payload"));
            }
            let mut section = &wasm[..section_len];
            wasm = &wasm[section_len..];

            if section_id == 0 {
                // Custom section
                let name_len = read_u32_leb128(&mut section)? as usize;
                if section.len() < name_len {
                    continue;
                }
                let Ok(name) = std::str::from_utf8(&section[..name_len]) else {
                    continue;
                };
                section = &section[name_len..];
                if name == "contractspecv0" {
                    let mut cursor = Cursor::new(section);
                    let mut limited = Limited::new(&mut cursor, Limits::none());
                    while let Ok(entry) = ScSpecEntry::read_xdr(&mut limited) {
                        if let ScSpecEntry::FunctionV0(f) = entry {
                            let fn_name = f.name.to_utf8_string_lossy();
                            if !fn_name.is_empty() && !spec_functions.contains(&fn_name) {
                                spec_functions.push(fn_name);
                            }
                        }
                    }
                }
            } else if section_id == 7 {
                // Export section
                let export_count = read_u32_leb128(&mut section)? as usize;
                for _ in 0..export_count {
                    if section.is_empty() {
                        break;
                    }
                    let name_len = read_u32_leb128(&mut section)? as usize;
                    if section.len() < name_len {
                        break;
                    }
                    let Ok(name) = std::str::from_utf8(&section[..name_len]) else {
                        break;
                    };
                    section = &section[name_len..];
                    if section.is_empty() {
                        break;
                    }
                    let kind = section[0];
                    section = &section[1..];
                    let _index = read_u32_leb128(&mut section)?;
                    // kind == 0 indicates exported function
                    if kind == 0
                        && !name.starts_with("__")
                        && name != "memory"
                        && name != "_"
                        && !name.starts_with('$')
                        && !exported_functions.iter().any(|s: &String| s == name)
                    {
                        exported_functions.push(name.to_string());
                    }
                }
            }
        }

        if !spec_functions.is_empty() {
            Ok(spec_functions)
        } else if !exported_functions.is_empty() {
            Ok(exported_functions)
        } else {
            Ok(Vec::new())
        }
    }

    /// Extract public functions from serialized `ScSpecEntry` bytes.
    pub fn extract_spec_xdr_functions(xdr_bytes: &[u8]) -> Result<Vec<String>, CoverageError> {
        let mut cursor = Cursor::new(xdr_bytes);
        let mut limited = Limited::new(&mut cursor, Limits::none());
        let mut fns = Vec::new();
        while let Ok(entry) = ScSpecEntry::read_xdr(&mut limited) {
            if let ScSpecEntry::FunctionV0(f) = entry {
                let name = f.name.to_utf8_string_lossy();
                if !name.is_empty() && !fns.contains(&name) {
                    fns.push(name);
                }
            }
        }
        Ok(fns)
    }

    /// Extract public functions from a [`ClientDefinition`].
    pub fn extract_client_functions(client_def: &ClientDefinition) -> Vec<String> {
        client_def.functions.clone()
    }

    /// Bind a human-readable alias to an address/contract key.
    pub fn bind_address(&mut self, name_or_key: &str, contract: impl AsContractKey) -> &mut Self {
        let target_key = contract.to_contract_key();
        self.aliases
            .insert(name_or_key.to_string(), target_key.clone());

        if let Some(mut state) = self.contracts.remove(name_or_key) {
            state.contract_id = target_key.clone();
            if state.name.is_none() {
                state.name = Some(name_or_key.to_string());
            }
            self.contracts.insert(target_key, state);
        }
        self
    }

    /// Exclude a specific function of a contract from the required coverage calculations.
    pub fn ignore_function(
        &mut self,
        contract: impl AsContractKey,
        function: impl Into<String>,
    ) -> &mut Self {
        let key = self.resolve_key(&contract.to_contract_key());
        let state = self
            .contracts
            .entry(key.clone())
            .or_insert_with(|| ContractState {
                contract_id: key,
                name: None,
                functions: BTreeSet::new(),
                ignored: BTreeSet::new(),
                invocations: BTreeMap::new(),
            });
        state.ignored.insert(function.into());
        self
    }

    /// Exclude multiple functions of a contract from required coverage calculations.
    pub fn ignore_functions(
        &mut self,
        contract: impl AsContractKey,
        functions: impl IntoIterator<Item = impl Into<String>>,
    ) -> &mut Self {
        let key = contract.to_contract_key();
        for func in functions {
            self.ignore_function(&key, func);
        }
        self
    }

    /// Ignore a function name globally across all monitored contracts (e.g. `"initialize"` or `"__check_auth"`).
    pub fn ignore_globally(&mut self, function: impl Into<String>) -> &mut Self {
        self.global_ignored.insert(function.into());
        self
    }

    /// Ignore standard initialization and setup entry points (`"init"`, `"initialize"`, `"__check_auth"`).
    pub fn ignore_default_setup_functions(&mut self, contract: impl AsContractKey) -> &mut Self {
        self.ignore_functions(contract, ["init", "initialize", "__check_auth"])
    }

    /// Remove a function from the contract's ignore list.
    pub fn unignore_function(&mut self, contract: impl AsContractKey, function: &str) -> &mut Self {
        let key = self.resolve_key(&contract.to_contract_key());
        if let Some(state) = self.contracts.get_mut(&key) {
            state.ignored.remove(function);
        }
        self
    }

    /// Manually record an invocation of a contract function.
    pub fn record_invocation(
        &mut self,
        contract: impl AsContractKey,
        function: impl Into<String>,
    ) -> &mut Self {
        self.record_invocations(contract, function, 1)
    }

    /// Manually record multiple invocations of a contract function.
    pub fn record_invocations(
        &mut self,
        contract: impl AsContractKey,
        function: impl Into<String>,
        count: usize,
    ) -> &mut Self {
        let key = contract.to_contract_key();
        let fn_name = function.into();
        self.record_invocation_internal(&key, &fn_name, count);
        self
    }

    /// Read the host's diagnostic events from `env` since the last observation and record invocations.
    pub fn observe(&mut self, env: &Env) -> &mut Self {
        let host_ptr = env.host() as *const _ as usize;
        let events: Vec<HostEvent> = match env.host().get_diagnostic_events() {
            Ok(events) => events.0,
            Err(_) => return self,
        };

        if let Some(prev) = self.last_observed_events.get(&host_ptr) {
            if prev == &events {
                return self;
            }
        }

        for host_event in &events {
            let event = &host_event.event;
            if event.type_ != ContractEventType::Diagnostic {
                continue;
            }
            let ContractEventBody::V0(body) = &event.body;
            let topics = &body.topics;
            let (Some(ScVal::Symbol(kind)), Some(contract_val), Some(ScVal::Symbol(function))) =
                (topics.first(), topics.get(1), topics.get(2))
            else {
                continue;
            };
            if kind.0.as_slice() != b"fn_call" {
                continue;
            }
            let contract_key = match contract_val {
                ScVal::Bytes(bytes) => {
                    let Ok(arr): Result<[u8; 32], _> = bytes.0.as_slice().try_into() else {
                        continue;
                    };
                    contract_strkey(&arr)
                }
                ScVal::Address(ScAddress::Contract(ContractId(Hash(bytes)))) => {
                    contract_strkey(bytes)
                }
                _ => continue,
            };
            let fn_name = String::from_utf8_lossy(function.0.as_slice()).into_owned();
            self.record_invocation_internal(&contract_key, &fn_name, 1);
        }

        self.last_observed_events.insert(host_ptr, events);
        self
    }

    /// Reset all tracked invocations and event log cursors while preserving contract registrations and ignore lists.
    pub fn reset_events(&mut self) -> &mut Self {
        for state in self.contracts.values_mut() {
            state.invocations.clear();
        }
        self.last_observed_events.clear();
        self
    }

    /// Reset the entire auditor back to its initial empty state.
    pub fn reset(&mut self) -> &mut Self {
        self.contracts.clear();
        self.aliases.clear();
        self.global_ignored.clear();
        self.last_observed_events.clear();
        self
    }

    /// Generate a comprehensive [`CoverageReport`] detailing execution metrics across all contracts.
    pub fn report(&self) -> CoverageReport {
        let mut contract_reports = BTreeMap::new();
        let mut total_functions = 0;
        let mut total_invoked = 0;
        let mut total_uninvoked = 0;
        let mut total_ignored = 0;

        for (contract_id, state) in &self.contracts {
            let mut function_names: BTreeSet<String> = state.functions.clone();
            function_names.extend(state.invocations.keys().cloned());

            let mut records = Vec::new();
            for name in &function_names {
                let is_ignored = state.ignored.contains(name) || self.global_ignored.contains(name);
                let invocations = state.invocations.get(name).copied().unwrap_or(0);
                let status = if is_ignored {
                    FunctionCoverageStatus::Ignored
                } else if invocations > 0 {
                    FunctionCoverageStatus::Invoked
                } else {
                    FunctionCoverageStatus::Uninvoked
                };
                records.push(FunctionCoverageRecord {
                    name: name.clone(),
                    invocations,
                    status,
                });
            }

            let c_total = records.len();
            let c_ignored = records
                .iter()
                .filter(|r| r.status == FunctionCoverageStatus::Ignored)
                .count();
            let c_invoked = records
                .iter()
                .filter(|r| r.status == FunctionCoverageStatus::Invoked)
                .count();
            let c_uninvoked = records
                .iter()
                .filter(|r| r.status == FunctionCoverageStatus::Uninvoked)
                .count();

            let c_actionable = c_total.saturating_sub(c_ignored);
            let c_percent = if c_actionable == 0 {
                100.0
            } else {
                let raw = (c_invoked as f64 / c_actionable as f64) * 100.0;
                (raw * 10.0).round() / 10.0
            };

            total_functions += c_total;
            total_invoked += c_invoked;
            total_uninvoked += c_uninvoked;
            total_ignored += c_ignored;

            contract_reports.insert(
                contract_id.clone(),
                ContractCoverageReport {
                    contract_id: contract_id.clone(),
                    contract_name: state.name.clone(),
                    total_functions: c_total,
                    invoked_count: c_invoked,
                    uninvoked_count: c_uninvoked,
                    ignored_count: c_ignored,
                    coverage_percent: c_percent,
                    functions: records,
                },
            );
        }

        let overall_actionable = total_functions.saturating_sub(total_ignored);
        let overall_percent = if overall_actionable == 0 {
            100.0
        } else {
            let raw = (total_invoked as f64 / overall_actionable as f64) * 100.0;
            (raw * 10.0).round() / 10.0
        };

        CoverageReport {
            total_contracts: self.contracts.len(),
            total_functions,
            total_invoked,
            total_uninvoked,
            total_ignored,
            overall_coverage_percent: overall_percent,
            contracts: contract_reports,
        }
    }

    /// Whether all actionable public contract functions were invoked during the audit.
    pub fn is_fully_covered(&self) -> bool {
        self.report().is_fully_covered()
    }

    fn resolve_key(&self, key: &str) -> String {
        self.aliases
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string())
    }

    fn record_invocation_internal(&mut self, key: &str, function: &str, count: usize) {
        let resolved = self.resolve_key(key);
        if let Some(state) = self.contracts.get_mut(&resolved) {
            *state.invocations.entry(function.to_string()).or_insert(0) += count;
            return;
        }

        // If not found by resolved key, search by contract_name
        for state in self.contracts.values_mut() {
            if state.name.as_deref() == Some(key) {
                *state.invocations.entry(function.to_string()).or_insert(0) += count;
                return;
            }
        }

        // If unregistered contract was invoked, auto-register as observed
        let state = self
            .contracts
            .entry(resolved.clone())
            .or_insert_with(|| ContractState {
                contract_id: resolved,
                name: None,
                functions: BTreeSet::new(),
                ignored: BTreeSet::new(),
                invocations: BTreeMap::new(),
            });
        *state.invocations.entry(function.to_string()).or_insert(0) += count;
    }
}

/// Helper function to parse an unsigned 32-bit LEB128 integer from a byte slice.
fn read_u32_leb128(cursor: &mut &[u8]) -> Result<u32, CoverageError> {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        if cursor.is_empty() {
            return Err(CoverageError::InvalidWasm(
                "unexpected end of WASM binary while reading LEB128 integer",
            ));
        }
        let byte = cursor[0];
        *cursor = &cursor[1..];
        result |= ((byte & 0x7F) as u32) << shift;
        if (byte & 0x80) == 0 {
            break;
        }
        shift += 7;
        if shift > 35 {
            return Err(CoverageError::InvalidWasm("LEB128 integer overflow"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::xdr::{ScSpecFunctionV0, ScSymbol, StringM, VecM, WriteXdr};
    use soroban_sdk::{contract, contractimpl};

    fn encode_u32_leb128(mut val: u32) -> Vec<u8> {
        let mut buf = Vec::new();
        loop {
            let mut byte = (val & 0x7F) as u8;
            val >>= 7;
            if val != 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if val == 0 {
                break;
            }
        }
        buf
    }

    fn build_wasm_with_spec(functions: &[&str]) -> Vec<u8> {
        let mut wasm = Vec::new();
        wasm.extend_from_slice(b"\0asm");
        wasm.extend_from_slice(&[1, 0, 0, 0]);

        let mut spec_bytes = Vec::new();
        for func in functions {
            let entry = ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
                doc: StringM::default(),
                name: ScSymbol((*func).try_into().unwrap()),
                inputs: VecM::default(),
                outputs: VecM::default(),
            });
            let xdr = entry.to_xdr(Limits::none()).unwrap();
            spec_bytes.extend_from_slice(&xdr);
        }

        let section_name = "contractspecv0";
        let name_len = encode_u32_leb128(section_name.len() as u32);
        let mut section_payload = Vec::new();
        section_payload.extend_from_slice(&name_len);
        section_payload.extend_from_slice(section_name.as_bytes());
        section_payload.extend_from_slice(&spec_bytes);

        wasm.push(0); // section id 0: custom
        wasm.extend_from_slice(&encode_u32_leb128(section_payload.len() as u32));
        wasm.extend_from_slice(&section_payload);

        wasm
    }

    fn build_wasm_with_exports(functions: &[&str]) -> Vec<u8> {
        let mut wasm = Vec::new();
        wasm.extend_from_slice(b"\0asm");
        wasm.extend_from_slice(&[1, 0, 0, 0]);

        let mut payload = Vec::new();
        // export count
        payload.extend_from_slice(&encode_u32_leb128(functions.len() as u32));
        for (i, func) in functions.iter().enumerate() {
            let name_bytes = func.as_bytes();
            payload.extend_from_slice(&encode_u32_leb128(name_bytes.len() as u32));
            payload.extend_from_slice(name_bytes);
            payload.push(0); // function export
            payload.extend_from_slice(&encode_u32_leb128(i as u32));
        }

        wasm.push(7); // section id 7: export
        wasm.extend_from_slice(&encode_u32_leb128(payload.len() as u32));
        wasm.extend_from_slice(&payload);

        wasm
    }

    #[contract]
    pub struct TestAuditedVault;

    #[contractimpl]
    impl TestAuditedVault {
        pub fn initialize(_env: Env) {}
        pub fn deposit(_env: Env) -> u32 {
            100
        }
        pub fn withdraw(_env: Env) -> u32 {
            50
        }
        pub fn emergency_pause(_env: Env) {}
        pub fn admin_recover(_env: Env) {}
    }

    #[test]
    fn test_extract_wasm_functions_from_contractspecv0() {
        let wasm = build_wasm_with_spec(&["init", "deposit", "withdraw", "emergency_stop"]);
        let extracted = CoverageAuditor::extract_wasm_functions(&wasm).unwrap();
        assert_eq!(
            extracted,
            vec!["init", "deposit", "withdraw", "emergency_stop"]
        );
    }

    #[test]
    fn test_extract_wasm_functions_from_export_section() {
        let wasm = build_wasm_with_exports(&["deposit", "withdraw", "emergency_pause"]);
        let extracted = CoverageAuditor::extract_wasm_functions(&wasm).unwrap();
        assert_eq!(extracted, vec!["deposit", "withdraw", "emergency_pause"]);
    }

    #[test]
    fn test_extract_spec_xdr_functions() {
        let mut spec_bytes = Vec::new();
        for func in &["swap", "add_liquidity", "remove_liquidity"] {
            let entry = ScSpecEntry::FunctionV0(ScSpecFunctionV0 {
                doc: StringM::default(),
                name: ScSymbol((*func).try_into().unwrap()),
                inputs: VecM::default(),
                outputs: VecM::default(),
            });
            spec_bytes.extend_from_slice(&entry.to_xdr(Limits::none()).unwrap());
        }

        let fns = CoverageAuditor::extract_spec_xdr_functions(&spec_bytes).unwrap();
        assert_eq!(fns, vec!["swap", "add_liquidity", "remove_liquidity"]);
    }

    #[test]
    fn test_client_definition_builder_and_macro() {
        let def1 = client_definition!("TokenContract", [initialize, transfer, balance]);
        assert_eq!(def1.contract_name, "TokenContract");
        assert_eq!(
            def1.public_functions(),
            &["initialize", "transfer", "balance"]
        );

        let def2 = ClientDefinition::new("Vault")
            .function("deposit")
            .functions(["withdraw", "lock"]);
        assert_eq!(def2.contract_name, "Vault");
        assert_eq!(def2.public_functions(), &["deposit", "withdraw", "lock"]);
    }

    #[test]
    fn test_observe_real_soroban_contract_invocations_and_uninvoked_detection() {
        let env = Env::default();
        let contract_id = env.register(TestAuditedVault, ());
        let client = TestAuditedVaultClient::new(&env, &contract_id);

        let mut auditor = CoverageAuditor::new();
        auditor.register_contract_with_name(
            "TestVault",
            &contract_id,
            [
                "initialize",
                "deposit",
                "withdraw",
                "emergency_pause",
                "admin_recover",
            ],
        );
        auditor.ignore_function(&contract_id, "initialize");

        // Execute only deposit and withdraw
        let _ = client.deposit();
        auditor.observe(&env);
        let _ = client.withdraw();
        auditor.observe(&env);

        let report = auditor.report();
        assert_eq!(report.total_contracts, 1);
        assert_eq!(report.total_functions, 5);
        assert_eq!(report.total_invoked, 2);
        assert_eq!(report.total_uninvoked, 2);
        assert_eq!(report.total_ignored, 1);
        // Actionable: 5 - 1 = 4. Invoked: 2. Coverage: 50.0%
        assert_eq!(report.overall_coverage_percent, 50.0);
        assert!(!report.is_fully_covered());

        let contract_rep = report.contract(&contract_id).unwrap();
        assert_eq!(contract_rep.coverage_percent, 50.0);
        assert_eq!(
            contract_rep.uninvoked_functions(),
            vec!["admin_recover", "emergency_pause"]
        );
        assert_eq!(
            contract_rep.invoked_functions(),
            vec!["deposit", "withdraw"]
        );
        assert_eq!(contract_rep.ignored_functions(), vec!["initialize"]);

        // Now exercise the remaining functions
        client.emergency_pause();
        auditor.observe(&env);
        client.admin_recover();
        auditor.observe(&env);

        let final_report = auditor.report();
        assert_eq!(final_report.total_invoked, 4);
        assert_eq!(final_report.total_uninvoked, 0);
        assert_eq!(final_report.overall_coverage_percent, 100.0);
        assert!(final_report.is_fully_covered());
        final_report.assert_full_coverage();
    }

    #[test]
    fn test_configurable_ignore_lists() {
        let mut auditor = CoverageAuditor::new();
        auditor.register_contract("C1", ["init", "setup", "run", "drain"]);

        // Ignore per-contract
        auditor.ignore_function("C1", "init");
        auditor.ignore_functions("C1", ["setup"]);

        // Ignore globally
        auditor.ignore_globally("drain");

        auditor.record_invocation("C1", "run");

        let report = auditor.report();
        let c1 = report.contract("C1").unwrap();
        assert_eq!(c1.invoked_count, 1);
        assert_eq!(c1.uninvoked_count, 0);
        assert_eq!(c1.ignored_count, 3);
        // Actionable: 4 - 3 = 1. Invoked: 1. Coverage: 100%
        assert_eq!(c1.coverage_percent, 100.0);
        assert!(c1.is_fully_covered());

        // Unignore drain
        auditor.unignore_function("C1", "init");
        let rep2 = auditor.report();
        let c1_rep2 = rep2.contract("C1").unwrap();
        assert_eq!(c1_rep2.uninvoked_count, 1); // init is now uninvoked
        assert_eq!(c1_rep2.coverage_percent, 50.0);
    }

    #[test]
    fn test_markdown_and_text_report_outputs() {
        let mut auditor = CoverageAuditor::new();
        auditor.register_contract_with_name("Vault", "CAAA111", ["deposit", "withdraw", "drain"]);
        auditor.ignore_function("CAAA111", "drain");
        auditor.record_invocations("CAAA111", "deposit", 5);

        let report = auditor.report();
        let md = report.to_markdown();
        assert!(md.contains("# Contract Function Coverage Report"));
        assert!(md.contains("`Vault` (`CAAA111`)"));
        assert!(md.contains("| `deposit` | ✓ Invoked | 5 | - |"));
        assert!(md.contains(
            "| `withdraw` | ✗ **Uninvoked** | 0 | **Dead code or unexercised entry point** |"
        ));
        assert!(md.contains(
            "| `drain` | ⊘ Ignored | 0 | Configured ignore list (setup/initialization) |"
        ));
        assert!(md.contains("> [!WARNING]"));

        let text = report.to_text();
        assert!(text.contains("CONTRACT FUNCTION COVERAGE REPORT"));
        assert!(text.contains("[x] deposit"));
        assert!(text.contains("[ ] withdraw"));
        assert!(text.contains("[-] drain"));
        assert!(text.contains("WARNING: 1 uninvoked contract function(s) detected:"));
    }

    #[test]
    #[should_panic(expected = "Contract function coverage assertion failed!")]
    fn test_assert_full_coverage_panics_on_uninvoked() {
        let mut auditor = CoverageAuditor::new();
        auditor.register_contract("C1", ["foo", "bar"]);
        auditor.record_invocation("C1", "foo");

        let report = auditor.report();
        report.assert_full_coverage();
    }

    #[test]
    #[should_panic(expected = "Contract function coverage below minimum threshold")]
    fn test_assert_min_coverage_panics_below_threshold() {
        let mut auditor = CoverageAuditor::new();
        auditor.register_contract("C1", ["f1", "f2", "f3", "f4"]);
        auditor.record_invocation("C1", "f1"); // 25%

        let report = auditor.report();
        report.assert_min_coverage(50.0);
    }

    #[test]
    fn test_invalid_wasm_error_handling() {
        let invalid = b"not a wasm binary";
        let err = CoverageAuditor::extract_wasm_functions(invalid).unwrap_err();
        assert!(matches!(err, CoverageError::InvalidWasm(_)));
    }
}
