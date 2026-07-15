//! Self-evolution system — typed memory, change audit, security, and skill creation.
//!
//! This module provides the infrastructure for the agent to evolve its own
//! capabilities over time through:
//!
//! - **Skill lifecycle**: [`Curator`] manages skill transitions (Candidate → Draft → Active → Stale → Deprecated → Archived)
//! - **Typed memory**: Structured metadata (type, confidence, stability, risk) for every memory
//! - **Change audit**: Complete log of all mutations with rollback capability
//! - **Security**: Secret scanning, untrusted input isolation, injection detection
//! - **Memory review**: Staleness scoring, conflict detection, merge, and archival
//! - **Skill creation**: Candidate detection from observed patterns, draft SKILL.md generation
//!
//! # Related Modules
//!
//! This module works closely with `trace` for execution evidence. Offline
//! evaluation and prompt optimization remain separate optional capabilities.
//!
//! # Safety
//!
//! All mutations to memories, skills, and rules are recorded in the audit log.
//! High-risk changes (rule promotion, skill merges) require human review.
//! Content from untrusted sources is never automatically promoted.

pub mod audit;
pub mod auto_memory;
pub mod background_review;
pub mod candidate;
pub mod curator;
pub mod draft;
pub mod dreaming;
pub mod health;
pub mod layer;
pub mod merge;
pub mod patch;
pub mod recall;
pub mod review;
pub mod runtime_integration;
pub mod security;
pub mod triggers;

pub use audit::{
    ChangeEntry, ChangeEntryBuilder, ChangeFilter, ChangeLog, ChangeType, EntityType,
    JsonlChangeLog,
};
pub use auto_memory::{
    AutoMemoryConfig, Observation, ObservationCategory, extract_observations,
    format_observations_for_memory, observation_memory_key, observation_memory_type,
    write_observations_to_memory_layer,
};
pub use background_review::{
    BackgroundReviewConfig, BackgroundReviewer, ReviewCandidate, ReviewCandidateKind, ReviewOutcome,
};
pub use candidate::{CandidateReport, SkillCandidate, SkillCandidateDetector};
pub use curator::{Curator, CuratorConfig, CuratorState, CuratorStatus, SkillLifecycle, SkillMeta};
pub use draft::{DraftResult, SkillDraftGenerator};
pub use dreaming::{Dreaming, DreamingAction, DreamingConfig, DreamingDecision, DreamingReport};
pub use health::{HealthBreakdown, HealthStatus, SkillHealthMonitor, SkillHealthReport};
pub use layer::{
    HotEntryMeta, LayerChangeResult, MemoryFile, MemoryLayer, MemoryLayerManager,
    MemoryWriteObserver,
};
pub use merge::{SimilarityBreakdown, SkillMergeProposal, SkillMerger, SkillSimilarityDetector};
pub use patch::{PatchType, SkillPatch, SkillPatcher};
pub use recall::MemoryRecaller;
pub use review::{
    AppliedMemoryMerge, ConflictDetector, ConflictGroup, MemoryConflictMember,
    MemoryConflictProposal, MemoryMergeSnapshot, MemoryMerger, MemoryReviewer, MergeResult,
    ReviewChange, ReviewConfig, ReviewReport, StalenessReport, StalenessScorer,
};
pub use runtime_integration::MemoryRuntimeIntegrationBuilder;
pub use security::{
    EvolutionSecurityGuard, InputTrustLevel, PromptInjectionDetector, ScanResult, SecretScanner,
    SecurityConfig, SecurityVerdict,
};
pub use triggers::{
    ExplicitSaveRecord, MemoryTriggerDisposition, MemoryTriggerSink, ToolFailureRecord,
    ToolSequenceRecord, ToolSuccessRecord, TriggerContext, TriggerDetector, TriggerEvidence,
    TriggerMatch,
};
