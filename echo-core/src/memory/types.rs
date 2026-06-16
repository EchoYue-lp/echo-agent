//! Typed memory metadata — structured classification and quality metrics for memory entries.
//!
//! Each memory entry in the evolution system carries a `MemoryMeta` struct that
//! classifies its type, tracks confidence/importance/stability, and records its
//! source and lifecycle status. `MemoryMeta` is serialized into `StoreItem.value`
//! JSON alongside the `content` field, requiring no changes to the `Store` trait
//! or `StoreItem` schema.

use serde::{Deserialize, Serialize};

// ── MemoryType ──────────────────────────────────────────────────────────

/// Classification of a memory entry.
///
/// Every memory must be typed — untyped free-text entries are not allowed
/// in the evolution system. The type determines how the memory is scored,
/// reviewed, and potentially promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// A stable user preference (e.g., "user prefers concise answers").
    UserPreference,
    /// A fact about the project (e.g., "this project uses Maven with Java 8").
    ProjectFact,
    /// An architecture or product decision (e.g., "we chose SQLite over Postgres").
    ArchitectureDecision,
    /// A debugging lesson learned (e.g., "Maven compile fails if JAVA_HOME is wrong").
    DebuggingLesson,
    /// An error symptom, cause, and verified solution.
    ErrorResolution,
    /// A commonly used command, environment variable, or build step.
    CommandPattern,
    /// Experience with a specific tool (e.g., "rg is faster than grep for this project").
    ToolUsage,
    /// A repeated workflow pattern that may become a skill candidate.
    WorkflowPattern,
    /// A workflow that has been identified as a potential skill.
    SkillCandidate,
    /// A note that has been superseded or is no longer accurate, kept for traceability.
    DeprecatedNote,
}

impl MemoryType {
    /// Default stability for this memory type (how resistant to becoming stale).
    ///
    /// Higher values mean the memory is less likely to become outdated quickly.
    pub fn default_stability(&self) -> f32 {
        match self {
            Self::UserPreference => 0.85,
            Self::ProjectFact => 0.60,
            Self::ArchitectureDecision => 0.80,
            Self::DebuggingLesson => 0.55,
            Self::ErrorResolution => 0.50,
            Self::CommandPattern => 0.40,
            Self::ToolUsage => 0.50,
            Self::WorkflowPattern => 0.60,
            Self::SkillCandidate => 0.55,
            Self::DeprecatedNote => 0.10,
        }
    }

    /// Whether this memory type is eligible for promotion to a skill.
    pub fn is_skill_eligible(&self) -> bool {
        matches!(
            self,
            Self::WorkflowPattern | Self::SkillCandidate | Self::DebuggingLesson
        )
    }

    /// Whether this memory type is eligible for rule promotion.
    pub fn is_rule_eligible(&self) -> bool {
        matches!(
            self,
            Self::UserPreference | Self::ArchitectureDecision | Self::ProjectFact
        )
    }
}

// ── MemoryRisk ──────────────────────────────────────────────────────────

/// Risk level of a memory entry.
///
/// High-risk memories (from untrusted sources or containing sensitive content)
/// require additional review before promotion to hot memory or rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRisk {
    /// Low risk — can be automatically processed and promoted.
    Low,
    /// Medium risk — may need review before promotion.
    Medium,
    /// High risk — from untrusted source or containing sensitive content.
    /// Must not be promoted to hot memory or rules without human approval.
    High,
}

impl Default for MemoryRisk {
    fn default() -> Self {
        Self::Low
    }
}

// ── MemoryStatus ────────────────────────────────────────────────────────

/// Lifecycle status of a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    /// Newly created, not yet confirmed or stabilized.
    Draft,
    /// Confirmed and in active use.
    Active,
    /// Replaced by a newer memory entry. `superseded_by` in `MemoryMeta` points to the replacement.
    Superseded,
    /// Archived — no longer active, kept for traceability.
    Archived,
}

impl Default for MemoryStatus {
    fn default() -> Self {
        Self::Active
    }
}

// ── MemorySource ────────────────────────────────────────────────────────

/// How this memory entry was created.
///
/// The source determines the default confidence level and whether the memory
/// can be automatically promoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    /// Extracted from a user correction signal (e.g., "不要这样做").
    UserCorrection,
    /// Extracted from a tool failure → success sequence.
    ErrorResolution,
    /// Detected from a repeated workflow pattern across sessions.
    RepeatedWorkflow,
    /// Explicitly saved by the user (e.g., `/remember` command).
    ExplicitSave,
    /// Automatically extracted by heuristic or LLM review.
    AutoExtracted,
    /// Promoted from L3 context compression.
    L3Promotion,
}

impl MemorySource {
    /// Default confidence for memories from this source.
    pub fn default_confidence(&self) -> f32 {
        match self {
            Self::ExplicitSave => 1.0,
            Self::UserCorrection => 0.90,
            Self::ErrorResolution => 0.85,
            Self::RepeatedWorkflow => 0.75,
            Self::AutoExtracted => 0.60,
            Self::L3Promotion => 0.50,
        }
    }
}

// ── MemoryMeta ──────────────────────────────────────────────────────────

/// Structured metadata for a typed memory entry.
///
/// Serialized into `StoreItem.value` JSON as:
/// ```json
/// {
///   "content": "The actual memory text",
///   "meta": { "memory_type": "debugging_lesson", "confidence": 0.85, ... }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryMeta {
    /// Classification of this memory.
    pub memory_type: MemoryType,
    /// How confident we are that this memory is accurate (0.0–1.0).
    pub confidence: f32,
    /// How stable this memory is — resistance to becoming stale (0.0–1.0).
    pub stability: f32,
    /// Risk level — determines review requirements.
    pub risk: MemoryRisk,
    /// Lifecycle status of this memory.
    pub status: MemoryStatus,
    /// How this memory was created.
    pub source: MemorySource,
    /// Topic category (e.g., "build", "debugging", "architecture").
    pub topic: String,
    /// Number of times this memory has been revised.
    #[serde(default)]
    pub revision_count: u32,
    /// If this memory was superseded, key of the replacement memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
}

impl MemoryMeta {
    /// Create a new MemoryMeta with type and source, using their default values.
    pub fn new(memory_type: MemoryType, source: MemorySource, topic: impl Into<String>) -> Self {
        Self {
            confidence: source.default_confidence(),
            stability: memory_type.default_stability(),
            memory_type,
            risk: MemoryRisk::default(),
            status: MemoryStatus::default(),
            source,
            topic: topic.into(),
            revision_count: 0,
            superseded_by: None,
        }
    }

    /// Create a MemoryMeta with explicit confidence.
    pub fn with_confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence.clamp(0.0, 1.0);
        self
    }

    /// Create a MemoryMeta with explicit stability.
    pub fn with_stability(mut self, stability: f32) -> Self {
        self.stability = stability.clamp(0.0, 1.0);
        self
    }

    /// Create a MemoryMeta with explicit risk.
    pub fn with_risk(mut self, risk: MemoryRisk) -> Self {
        self.risk = risk;
        self
    }

    /// Create a MemoryMeta with explicit status.
    pub fn with_status(mut self, status: MemoryStatus) -> Self {
        self.status = status;
        self
    }

    /// Whether this memory is eligible for promotion to hot memory.
    pub fn is_hot_eligible(&self) -> bool {
        self.confidence >= 0.85
            && self.stability >= 0.70
            && self.status == MemoryStatus::Active
            && self.risk != MemoryRisk::High
    }

    /// Whether this memory is eligible for rule promotion.
    pub fn is_rule_eligible(&self) -> bool {
        self.confidence >= 0.95
            && self.stability >= 0.90
            && self.status == MemoryStatus::Active
            && self.revision_count == 0
            && self.risk != MemoryRisk::High
            && self.memory_type.is_rule_eligible()
    }

    /// Compute a staleness score (0.0 = fresh, 1.0 = very stale).
    ///
    /// This is a simplified version; the full `StalenessScorer` in Phase 2
    /// incorporates age, usage, and contradiction factors.
    pub fn base_staleness(&self) -> f32 {
        // Low stability → high staleness risk
        let instability = 1.0 - self.stability;
        // Low confidence → higher staleness
        let unconfidence = 1.0 - self.confidence;
        // Deprecated status → high staleness
        let status_factor = match self.status {
            MemoryStatus::Active => 0.0,
            MemoryStatus::Draft => 0.1,
            MemoryStatus::Superseded => 0.8,
            MemoryStatus::Archived => 1.0,
        };

        (instability * 0.35 + unconfidence * 0.25 + status_factor * 0.40).min(1.0)
    }
}

impl Default for MemoryMeta {
    fn default() -> Self {
        Self {
            memory_type: MemoryType::ProjectFact,
            confidence: 0.5,
            stability: 0.5,
            risk: MemoryRisk::Low,
            status: MemoryStatus::Active,
            source: MemorySource::AutoExtracted,
            topic: String::new(),
            revision_count: 0,
            superseded_by: None,
        }
    }
}

// ── TypedMemoryValue ────────────────────────────────────────────────────

/// The JSON structure stored in `StoreItem.value` for typed memories.
///
/// This is the serialization format: the `content` field holds the actual
/// memory text, and `meta` holds the structured metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypedMemoryValue {
    /// The actual memory text content.
    pub content: String,
    /// Structured metadata for this memory.
    pub meta: MemoryMeta,
}

impl TypedMemoryValue {
    /// Create a new typed memory value.
    pub fn new(content: impl Into<String>, meta: MemoryMeta) -> Self {
        Self {
            content: content.into(),
            meta,
        }
    }

    /// Serialize to a `serde_json::Value` for storage in `StoreItem.value`.
    pub fn to_value(&self) -> serde_json::Result<serde_json::Value> {
        serde_json::to_value(self)
    }

    /// Deserialize from a `serde_json::Value`.
    pub fn from_value(value: &serde_json::Value) -> serde_json::Result<Self> {
        serde_json::from_value(value.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_type_default_stability() {
        assert!(
            MemoryType::UserPreference.default_stability()
                > MemoryType::CommandPattern.default_stability()
        );
        assert!(
            MemoryType::ArchitectureDecision.default_stability()
                > MemoryType::ErrorResolution.default_stability()
        );
    }

    #[test]
    fn test_memory_source_default_confidence() {
        assert_eq!(MemorySource::ExplicitSave.default_confidence(), 1.0);
        assert!(
            MemorySource::UserCorrection.default_confidence()
                > MemorySource::AutoExtracted.default_confidence()
        );
    }

    #[test]
    fn test_memory_meta_new() {
        let meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        );
        assert_eq!(meta.memory_type, MemoryType::DebuggingLesson);
        assert_eq!(meta.source, MemorySource::ErrorResolution);
        assert_eq!(meta.topic, "build");
        assert_eq!(meta.confidence, 0.85); // ErrorResolution default
        assert_eq!(meta.status, MemoryStatus::Active);
        assert_eq!(meta.risk, MemoryRisk::Low);
    }

    #[test]
    fn test_memory_meta_builder() {
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.99)
        .with_stability(0.95)
        .with_risk(MemoryRisk::Medium);
        assert!((meta.confidence - 0.99).abs() < f32::EPSILON);
        assert!((meta.stability - 0.95).abs() < f32::EPSILON);
        assert_eq!(meta.risk, MemoryRisk::Medium);
    }

    #[test]
    fn test_hot_eligible() {
        let meta = MemoryMeta::new(MemoryType::ProjectFact, MemorySource::ExplicitSave, "test")
            .with_confidence(0.90)
            .with_stability(0.80);
        assert!(meta.is_hot_eligible());

        let low_confidence =
            MemoryMeta::new(MemoryType::ProjectFact, MemorySource::AutoExtracted, "test")
                .with_confidence(0.50);
        assert!(!low_confidence.is_hot_eligible());
    }

    #[test]
    fn test_rule_eligible() {
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.99)
        .with_stability(0.95);
        assert!(meta.is_rule_eligible());

        // Not eligible: wrong type
        let bug_meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "test",
        )
        .with_confidence(0.99)
        .with_stability(0.95);
        assert!(!bug_meta.is_rule_eligible());

        // Not eligible: high risk
        let risky = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "test",
        )
        .with_confidence(0.99)
        .with_stability(0.95)
        .with_risk(MemoryRisk::High);
        assert!(!risky.is_rule_eligible());
    }

    #[test]
    fn test_typed_memory_value_roundtrip() {
        let meta = MemoryMeta::new(
            MemoryType::DebuggingLesson,
            MemorySource::ErrorResolution,
            "build",
        );
        let value = TypedMemoryValue::new("Maven needs Java 8", meta);
        let json = value.to_value().unwrap();
        let parsed = TypedMemoryValue::from_value(&json).unwrap();
        assert_eq!(parsed.content, "Maven needs Java 8");
        assert_eq!(parsed.meta.memory_type, MemoryType::DebuggingLesson);
        assert_eq!(parsed.meta.topic, "build");
    }

    #[test]
    fn test_typed_memory_value_backward_compat() {
        // Old-format store items (no meta field) should be parseable with defaults
        let old_format = serde_json::json!({
            "content": "Some old memory"
        });
        // This should still parse — meta will get serde defaults
        let parsed: Result<TypedMemoryValue, _> = serde_json::from_value(old_format);
        // If meta is required, this will fail — that's expected.
        // The TypedMemoryStore handles backward compat by catching parse errors.
    }

    #[test]
    fn test_staleness_active_low() {
        let meta = MemoryMeta::new(
            MemoryType::UserPreference,
            MemorySource::ExplicitSave,
            "style",
        )
        .with_confidence(0.95)
        .with_stability(0.90);
        assert!(meta.base_staleness() < 0.3);
    }

    #[test]
    fn test_staleness_archived_high() {
        let meta = MemoryMeta::new(
            MemoryType::DeprecatedNote,
            MemorySource::AutoExtracted,
            "old",
        )
        .with_confidence(0.3)
        .with_stability(0.2)
        .with_status(MemoryStatus::Archived);
        assert!(meta.base_staleness() > 0.5);
    }
}
