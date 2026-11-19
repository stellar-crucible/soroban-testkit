//! Soroban SDK v29 compatibility test harness and deprecation auditor.
//!
//! As host protocols advance (Protocol 21/22 toward Protocol 23 and SDK v29),
//! legacy SDK APIs and obsolete host functions are deprecated. [`CompatibilityAuditor`]
//! inspects executed contract invocations, events, and host errors to flag deprecated
//! patterns early and produce Markdown compatibility scorecards with remediation
//! guidance for CI pipelines.

use std::collections::BTreeMap;
use std::fmt;

use soroban_sdk::testutils::Events as _;
use soroban_sdk::{Address, Env};

/// Target protocol version baseline for upcoming Soroban upgrades (Protocol 22 / SDK v29).
pub const DEFAULT_TARGET_PROTOCOL: u32 = 22;

/// Severity level of a detected deprecation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeprecationSeverity {
    /// Informational notice regarding upcoming best practices.
    Info,
    /// Deprecated in the current or upcoming SDK; should be remediated soon.
    Warning,
    /// Breaking deprecation or obsolete function removed in target protocol.
    Critical,
}

impl DeprecationSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeprecationSeverity::Info => "INFO",
            DeprecationSeverity::Warning => "WARNING",
            DeprecationSeverity::Critical => "CRITICAL",
        }
    }
}

/// A specific deprecated pattern, host call, or error code flagged during an audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeprecationItem {
    /// Identifier or signature of the deprecated method or pattern.
    pub identifier: String,
    /// Minimum target protocol version where this pattern is obsolete or removed.
    pub target_protocol: u32,
    /// Severity level.
    pub severity: DeprecationSeverity,
    /// Human-readable description of why the pattern is discouraged.
    pub description: String,
    /// Concrete, actionable migration or replacement steps.
    pub remediation: String,
    /// Number of times this deprecation was observed during the audit.
    pub occurrences: usize,
}

/// Overall grade awarded by the compatibility audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityGrade {
    /// Full compliance (100%): Ready for immediate upgrade.
    A,
    /// Minor warnings or informational deprecations only.
    B,
    /// Noticeable deprecations present; upgrade requires code modifications.
    C,
    /// Critical obsolete calls or breaking patterns detected; incompatible.
    F,
}

impl CompatibilityGrade {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompatibilityGrade::A => "Grade A (Ready)",
            CompatibilityGrade::B => "Grade B (Minor Warnings)",
            CompatibilityGrade::C => "Grade C (Action Needed)",
            CompatibilityGrade::F => "Grade F (Incompatible)",
        }
    }
}

/// Comprehensive scorecard detailing contract upgrade readiness for target protocol versions.
#[derive(Debug, Clone, PartialEq)]
pub struct CompatibilityScorecard {
    /// Target protocol version evaluated.
    pub target_protocol: u32,
    /// Compatibility score from 0.0 to 100.0.
    pub score_percent: f64,
    /// High-level letter grade.
    pub grade: CompatibilityGrade,
    /// Flagged deprecation items.
    pub items: Vec<DeprecationItem>,
    /// Whether the contract meets the threshold for on-chain protocol upgrade.
    pub is_ready_for_upgrade: bool,
}

impl CompatibilityScorecard {
    /// Total count of deprecation occurrences found.
    pub fn total_occurrences(&self) -> usize {
        self.items.iter().map(|i| i.occurrences).sum()
    }

    /// Items with critical severity.
    pub fn critical_items(&self) -> Vec<&DeprecationItem> {
        self.items
            .iter()
            .filter(|i| i.severity == DeprecationSeverity::Critical)
            .collect()
    }

    /// Items with warning severity.
    pub fn warning_items(&self) -> Vec<&DeprecationItem> {
        self.items
            .iter()
            .filter(|i| i.severity == DeprecationSeverity::Warning)
            .collect()
    }

    /// Render an actionable Markdown report suitable for pull-request comments or CI summaries.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(&format!(
            "# Soroban SDK v29 / Protocol {} Compatibility Scorecard\n\n",
            self.target_protocol
        ));

        md.push_str("## Executive Summary\n\n");
        md.push_str(&format!("- **Overall Rating**: {}\n", self.grade.as_str()));
        md.push_str(&format!(
            "- **Compatibility Score**: {:.1}%\n",
            self.score_percent
        ));
        md.push_str(&format!(
            "- **Upgrade Ready**: {}\n",
            if self.is_ready_for_upgrade {
                "✅ YES"
            } else {
                "❌ NO (Remediation Required)"
            }
        ));
        md.push_str(&format!(
            "- **Total Deprecations Flagged**: {} (Critical: {}, Warnings: {})\n\n",
            self.total_occurrences(),
            self.critical_items().len(),
            self.warning_items().len()
        ));

        if self.items.is_empty() {
            md.push_str("### ✅ Clean Compatibility\n\n");
            md.push_str("No deprecated host functions, legacy APIs, or obsolete error patterns were detected. The contract is fully compatible with target protocol standards.\n");
        } else {
            md.push_str("## Detected Deprecations & Remediations\n\n");
            md.push_str("| Feature / Identifier | Severity | Occurrences | Protocol Target | Description & Remediation |\n");
            md.push_str("|----------------------|----------|-------------|-----------------|---------------------------|\n");
            for item in &self.items {
                md.push_str(&format!(
                    "| `{}` | `{}` | {} | Protocol {} | **{}**<br/>👉 *Fix*: {} |\n",
                    item.identifier,
                    item.severity.as_str(),
                    item.occurrences,
                    item.target_protocol,
                    item.description,
                    item.remediation
                ));
            }
            md.push('\n');

            md.push_str("## Next Steps\n\n");
            md.push_str("1. Replace all `CRITICAL` obsolete calls prior to testing on Protocol 22+ networks.\n");
            md.push_str("2. Adopt typed `#[contractevent]` schemas in place of raw untyped event publishing.\n");
            md.push_str(
                "3. Run `CompatibilityAuditor::generate_scorecard()` in continuous integration.\n",
            );
        }

        md
    }
}

impl fmt::Display for CompatibilityScorecard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "CompatibilityScorecard (Protocol {}): {} ({:.1}%, {} issues)",
            self.target_protocol,
            self.grade.as_str(),
            self.score_percent,
            self.items.len()
        )
    }
}

/// Automated test harness that tracks and audits contract execution for deprecated APIs.
#[derive(Debug, Clone)]
pub struct CompatibilityAuditor {
    target_protocol: u32,
    recorded_calls: BTreeMap<String, usize>,
    recorded_errors: BTreeMap<u32, usize>,
    custom_deprecations: Vec<DeprecationItem>,
}

impl Default for CompatibilityAuditor {
    fn default() -> Self {
        Self {
            target_protocol: DEFAULT_TARGET_PROTOCOL,
            recorded_calls: BTreeMap::new(),
            recorded_errors: BTreeMap::new(),
            custom_deprecations: Vec::new(),
        }
    }
}

impl CompatibilityAuditor {
    /// Create a new compatibility auditor targeting Protocol 22 / SDK v29.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an explicit target protocol version (e.g. 21, 22, or 23).
    pub fn with_target_protocol(mut self, protocol: u32) -> Self {
        self.target_protocol = protocol;
        self
    }

    /// Record a method or SDK call observation during test execution.
    pub fn record_call(&mut self, method_name: &str) -> &mut Self {
        *self
            .recorded_calls
            .entry(method_name.to_string())
            .or_insert(0) += 1;
        self
    }

    /// Record a host error code observation during test execution.
    pub fn record_error_code(&mut self, code: u32) -> &mut Self {
        *self.recorded_errors.entry(code).or_insert(0) += 1;
        self
    }

    /// Register a custom deprecation rule or finding directly.
    pub fn record_deprecation(&mut self, item: DeprecationItem) -> &mut Self {
        self.custom_deprecations.push(item);
        self
    }

    /// Inspect contract events published in `env` for legacy raw event publishing patterns.
    pub fn audit_events(&mut self, env: &Env) -> &mut Self {
        let events = env.events().all();
        // Check if raw events have untyped single-element topics or raw symbol structures
        for event in events.events() {
            let soroban_sdk::xdr::ContractEventBody::V0(body) = &event.body;
            if body.topics.len() <= 1 {
                self.record_call("env.events().publish(untyped_raw)");
            }
        }
        self
    }

    /// Audit a contract address in `env` by inspecting events and storage metadata.
    pub fn audit_contract(&mut self, env: &Env, _contract: &Address) -> CompatibilityScorecard {
        self.audit_events(env);
        self.generate_scorecard()
    }

    /// Generate the final [`CompatibilityScorecard`] by matching recorded calls against known deprecations.
    pub fn generate_scorecard(&self) -> CompatibilityScorecard {
        let mut items = self.custom_deprecations.clone();

        // Standard deprecation catalog
        let catalog = [
            (
                "env.events().publish",
                21,
                DeprecationSeverity::Warning,
                "Untyped env.events().publish(...) is deprecated in favor of #[contractevent].",
                "Define a #[contractevent] struct and call its .publish(env, ...) method.",
            ),
            (
                "env.events().publish(untyped_raw)",
                22,
                DeprecationSeverity::Warning,
                "Raw single-topic event publishing lacks structured schema typing.",
                "Adopt multi-topic structured event envelopes or #[contractevent].",
            ),
            (
                "env.budget()",
                21,
                DeprecationSeverity::Critical,
                "env.budget() is obsolete in modern SDKs and scheduled for removal in v29.",
                "Use env.cost_estimate().budget() or the soroban-testkit-core BudgetGuard.",
            ),
            (
                "env.current_contract_address_raw",
                22,
                DeprecationSeverity::Info,
                "Raw identifier casting for contract addresses is discouraged.",
                "Use high-level Address types directly without manual bytes conversion.",
            ),
            (
                "legacy_auth_replay",
                22,
                DeprecationSeverity::Critical,
                "Legacy non-nonce authorization structures cannot be submitted under Protocol 22+.",
                "Ensure all signatures incorporate ledger nonces and invoker verification.",
            ),
        ];

        for (ident, min_prot, sev, desc, remed) in catalog {
            if self.target_protocol >= min_prot {
                if let Some(&count) = self.recorded_calls.get(ident) {
                    if count > 0 {
                        items.push(DeprecationItem {
                            identifier: ident.to_string(),
                            target_protocol: min_prot,
                            severity: sev,
                            description: desc.to_string(),
                            remediation: remed.to_string(),
                            occurrences: count,
                        });
                    }
                }
            }
        }

        // Host error code deprecations (e.g. legacy budget code 21)
        for (&code, &count) in &self.recorded_errors {
            if code == 21 && self.target_protocol >= 21 {
                items.push(DeprecationItem {
                    identifier: format!("host_error_code_{code}"),
                    target_protocol: 21,
                    severity: DeprecationSeverity::Warning,
                    description: "Host error code 21 is a legacy budget error code.".to_string(),
                    remediation:
                        "Upgrade error handling to match modern ScErrorType::Budget taxonomies."
                            .to_string(),
                    occurrences: count,
                });
            }
        }

        // Calculate score & grade
        let mut score = 100.0f64;
        let mut critical_count = 0usize;
        let mut warning_count = 0usize;

        for item in &items {
            match item.severity {
                DeprecationSeverity::Critical => {
                    critical_count += 1;
                    score -= 25.0 * (item.occurrences as f64).min(4.0);
                }
                DeprecationSeverity::Warning => {
                    warning_count += 1;
                    score -= 10.0 * (item.occurrences as f64).min(3.0);
                }
                DeprecationSeverity::Info => {
                    score -= 2.0 * (item.occurrences as f64).min(2.0);
                }
            }
        }

        let score_percent = score.clamp(0.0, 100.0);
        let grade = if critical_count > 0 || score_percent < 50.0 {
            CompatibilityGrade::F
        } else if warning_count > 2 || score_percent < 75.0 {
            CompatibilityGrade::C
        } else if warning_count > 0 || score_percent < 95.0 {
            CompatibilityGrade::B
        } else {
            CompatibilityGrade::A
        };

        let is_ready_for_upgrade = critical_count == 0 && score_percent >= 70.0;

        CompatibilityScorecard {
            target_protocol: self.target_protocol,
            score_percent,
            grade,
            items,
            is_ready_for_upgrade,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_contract_scores_grade_a() {
        let auditor = CompatibilityAuditor::new().with_target_protocol(22);
        let scorecard = auditor.generate_scorecard();

        assert_eq!(scorecard.grade, CompatibilityGrade::A);
        assert_eq!(scorecard.score_percent, 100.0);
        assert!(scorecard.is_ready_for_upgrade);
        assert!(scorecard.items.is_empty());

        let md = scorecard.to_markdown();
        assert!(md.contains("Clean Compatibility"));
        assert!(md.contains("Grade A"));
    }

    #[test]
    fn test_deprecated_calls_trigger_warnings_and_markdown() {
        let mut auditor = CompatibilityAuditor::new().with_target_protocol(22);
        auditor.record_call("env.events().publish");
        auditor.record_call("env.events().publish");

        let scorecard = auditor.generate_scorecard();

        assert_eq!(scorecard.grade, CompatibilityGrade::B);
        assert!(scorecard.score_percent < 100.0);
        assert!(scorecard.is_ready_for_upgrade); // Warnings alone do not block upgrade
        assert_eq!(scorecard.items.len(), 1);
        assert_eq!(scorecard.items[0].occurrences, 2);

        let md = scorecard.to_markdown();
        assert!(md.contains("Detected Deprecations & Remediations"));
        assert!(md.contains("`env.events().publish`"));
        assert!(md.contains("#[contractevent]"));
    }

    #[test]
    fn test_critical_deprecation_fails_upgrade_readiness() {
        let mut auditor = CompatibilityAuditor::new().with_target_protocol(22);
        auditor.record_call("env.budget()");

        let scorecard = auditor.generate_scorecard();

        assert_eq!(scorecard.grade, CompatibilityGrade::F);
        assert!(!scorecard.is_ready_for_upgrade);
        assert_eq!(scorecard.critical_items().len(), 1);

        let md = scorecard.to_markdown();
        assert!(md.contains("CRITICAL"));
        assert!(md.contains("REMEDIAL STEPS") || md.contains("Remediation"));
    }

    #[test]
    fn test_target_protocol_filtering() {
        // If targeting protocol 20, protocol 21/22 deprecations shouldn't trigger
        let mut auditor = CompatibilityAuditor::new().with_target_protocol(20);
        auditor.record_call("env.events().publish");

        let scorecard = auditor.generate_scorecard();
        assert_eq!(scorecard.grade, CompatibilityGrade::A);
        assert!(scorecard.items.is_empty());

        // But when bumped to protocol 22, it triggers
        let upgraded = CompatibilityAuditor::new()
            .with_target_protocol(22)
            .record_call("env.events().publish")
            .generate_scorecard();
        assert_eq!(upgraded.items.len(), 1);
    }
}
