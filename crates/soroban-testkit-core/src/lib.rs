pub mod budget;
pub mod compatibility;
pub mod error;
pub mod footprint;
pub mod namespace_auditor;
pub mod profiler;
pub mod stack_depth;
pub mod storage;
pub mod wasm;
pub mod storage_fragmentation;

pub use compatibility::{
    CompatibilityAuditor, CompatibilityGrade, CompatibilityScorecard, DeprecationItem,
    DeprecationSeverity, DEFAULT_TARGET_PROTOCOL,
};
pub use footprint::{
    FootprintSnapshot, RentEstimate, RentParameters, StorageFootprint, StorageFootprintDiff,
    DEFAULT_PROJECTION_LEDGERS,
};
pub use namespace_auditor::{NamespaceAuditor, NamespaceReport};
pub use stack_depth::{
    CallFrame, StackDepthAuditor, StackDepthBreach, DEFAULT_WARNING_THRESHOLD,
    HOST_CALL_STACK_LIMIT,
};
pub use storage::{assert_storage_tier, StorageTier};
pub use storage_fragmentation::{
    ConsolidationOpportunity, FragmentationReport, StorageEntryMetrics,
    StorageFragmentationAnalyzer, TierFragmentationSummary,
};
