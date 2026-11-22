pub mod budget;
pub mod budget_report;
pub mod compatibility;
pub mod conservation;
pub mod coverage;
pub mod error;
pub mod footprint;
pub mod memory;
pub mod memory_profiler;
pub mod namespace_auditor;
pub mod reentrancy;
pub mod stack_depth;
pub mod storage;
pub mod storage_fragmentation;
pub mod tracer;
pub mod ttl;

pub use budget_report::{BudgetComparison, DiffStatus, FunctionBudgetDiff};
pub use compatibility::{
    CompatibilityAuditor, CompatibilityGrade, CompatibilityScorecard, DeprecationItem,
    DeprecationSeverity, DEFAULT_TARGET_PROTOCOL,
};
pub use conservation::{ConservationReport, ConservationVerifier, ReconciliationRow};
pub use coverage::{
    ClientDefinition, ContractCoverageReport, CoverageAuditor, CoverageError, CoverageReport,
    FunctionCoverageRecord, FunctionCoverageStatus,
};
pub use footprint::{
    FootprintSnapshot, RentEstimate, RentParameters, StorageFootprint, StorageFootprintDiff,
    DEFAULT_PROJECTION_LEDGERS,
};
pub use memory::{AllocationProfile, AllocationReport, MemoryPressureAuditor};
pub use memory_profiler::{MemoryProfileReport, MemoryProfiler, MemoryStep};
pub use namespace_auditor::{NamespaceAuditor, NamespaceReport};
pub use reentrancy::{
    ActionKind, LockState, ReentrancyGuardAuditor, ReentrancyReport, ReentrancyViolation,
    ReentrancyViolationKind, TimelineStep,
};
pub use stack_depth::{
    CallFrame, StackDepthAuditor, StackDepthBreach, DEFAULT_WARNING_THRESHOLD,
    HOST_CALL_STACK_LIMIT,
};
pub use storage::{assert_storage_tier, StorageTier};
pub use storage_fragmentation::{
    ConsolidationOpportunity, FragmentationReport, StorageEntryMetrics,
    StorageFragmentationAnalyzer, TierFragmentationSummary,
};
pub use tracer::{CallOutcome, CallTracer, TraceFrame};
pub use ttl::{RentThresholds, TtlExtension, TtlInspector, TtlStatus};
