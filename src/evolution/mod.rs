//! Self-evolution system — typed memory, change audit, security, and skill creation.
//!
//! This module provides the infrastructure for the agent to evolve its own
//! capabilities over time through:
//!
//! - **Skill lifecycle**: [`SkillMutationAuthority`] durably applies approved
//!   Curator + exact SKILL.md transitions (Candidate → Draft → Active → Stale → Deprecated → Archived)
//! - **Typed memory**: Structured metadata, exact source evidence, and
//!   explicit Draft-to-Active activation for reviewed long-term memory
//! - **Change audit**: Append-only evidence for durable, generation-fenced
//!   memory and Skill owner-applied rollback
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
//! Layered memory mutations carry durable audit identities; skill and rule
//! mutation coverage is tracked separately.
//! High-risk changes (rule promotion, skill merges) require human review.
//! Content from untrusted sources is never automatically promoted.

pub mod audit;
pub mod background_review;
pub mod candidate;
pub mod curator;
pub mod draft;
pub mod dreaming;
pub mod health;
pub mod layer;
pub mod merge;
mod mutation;
pub mod patch;
pub mod recall;
pub mod review;
pub mod runtime_integration;
pub mod security;
pub mod skill_mutation;
pub mod triggers;

pub use audit::{
    ChangeEntry, ChangeEntryBuilder, ChangeFilter, ChangeLog, ChangeRecordOutcome, ChangeType,
    EntityType, JsonlChangeLog,
};
pub use background_review::{
    BackgroundReviewConfig, BackgroundReviewer, ReviewCandidate, ReviewCandidateKind, ReviewOutcome,
};
pub use candidate::{CandidateReport, SkillCandidate, SkillCandidateDetector};
pub use curator::{Curator, CuratorConfig, CuratorState, CuratorStatus, SkillLifecycle, SkillMeta};
pub use draft::{DraftResult, SkillDraftGenerator, SkillDraftPreview};
pub use dreaming::{Dreaming, DreamingAction, DreamingConfig, DreamingDecision, DreamingReport};
pub use health::{HealthBreakdown, HealthStatus, SkillHealthMonitor, SkillHealthReport};
pub use layer::{
    EvolutionObserver, HotEntryMeta, LayerChangeResult, MemoryActivationOutcome,
    MemoryActivationProposal, MemoryActivationReceipt, MemoryFile, MemoryLayer, MemoryLayerManager,
    MemoryRollbackConflict, MemoryRollbackHistoryUnavailable, MemoryRollbackOutcome,
    MemoryRollbackPreview, MemoryRollbackPreviewOutcome, MemoryRollbackReceipt,
    MemoryRollbackTarget, is_stale_memory_proposal_error,
};
pub use merge::{
    SimilarityBreakdown, SkillMergePreview, SkillMergeProposal, SkillMerger,
    SkillSimilarityDetector,
};
pub use patch::{PatchType, SkillPatch, SkillPatchPreview, SkillPatcher};
pub use recall::MemoryRecaller;
pub use review::{
    AppliedMemoryMerge, ConflictDetector, ConflictGroup, MemoryConflictMember,
    MemoryConflictProposal, MemoryMergeSnapshot, MemoryMerger, MemoryReviewer, MergeResult,
    ReviewChange, ReviewConfig, ReviewReport, StalenessReport, StalenessScorer,
};
pub use runtime_integration::{HookEvolutionObserver, MemoryRuntimeIntegrationBuilder};
pub use security::{
    EvolutionSecurityGuard, InputTrustLevel, PromptInjectionDetector, ScanResult, SecretScanner,
    SecurityConfig, SecurityVerdict,
};
pub use skill_mutation::{
    SkillApprovalArtifact, SkillFileMutation, SkillMutationAuthority, SkillMutationKind,
    SkillMutationObserver, SkillMutationOutcome, SkillMutationPreview, SkillMutationReceipt,
    SkillMutationRequest, SkillRollbackLineage, SkillRollbackPreviewOutcome, SkillRollbackSupport,
    SkillRollbackTarget, SkillUsageHandle, SkillUsageOutcome,
};
pub use triggers::{
    ExplicitSaveRecord, MemoryTriggerDisposition, MemoryTriggerSink, ToolFailureRecord,
    ToolSequenceRecord, ToolSuccessRecord, TriggerContext, TriggerDetector, TriggerEvidence,
    TriggerMatch,
};
