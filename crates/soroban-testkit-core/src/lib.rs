pub mod budget;
pub mod compatibility;
pub mod error;
pub mod footprint;
pub mod namespace_auditor;
pub mod storage;
pub mod storage_fragmentation;
pub mod conservation;

pub use compatibility::{
    CompatibilityAuditor, CompatibilityGrade, CompatibilityScorecard, DeprecationItem,
    DeprecationSeverity, DEFAULT_TARGET_PROTOCOL,
};
pub use footprint::{FootprintSnapshot, StorageFootprint, StorageFootprintDiff};
pub use namespace_auditor::{NamespaceAuditor, NamespaceReport};
pub use storage::{assert_storage_tier, StorageTier};
pub use storage_fragmentation::{
    ConsolidationOpportunity, FragmentationReport, StorageEntryMetrics,
    StorageFragmentationAnalyzer, TierFragmentationSummary,
};
pub use conservation::{ConservationVerifier, ReconciliationEntry, ReconciliationReport};
