pub mod budget;
pub mod error;
pub mod footprint;
pub mod namespace_auditor;
pub mod storage;
pub mod storage_fragmentation;

pub use footprint::{FootprintSnapshot, StorageFootprint, StorageFootprintDiff};
pub use namespace_auditor::{NamespaceAuditor, NamespaceReport};
pub use storage::{assert_storage_tier, StorageTier};
pub use storage_fragmentation::{
    ConsolidationOpportunity, FragmentationReport, StorageEntryMetrics,
    StorageFragmentationAnalyzer, TierFragmentationSummary,
};
