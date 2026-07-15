//! Write trigger detection — identifies when agent-learned memories should be persisted.
//!
//! The `TriggerDetector` scans conversational context for signals that indicate
//! a memory-worthy moment has occurred. All detection methods are **synchronous**
//! (pattern matching only); the caller is responsible for async persistence.
//!
//! ## Supported triggers
//!
//! | Trigger | Pattern | Default confidence | MemoryType |
//! |---------|---------|-------------------|------------|
//! | `UserCorrection` | "不是这样"/"不对"/"不要"/"wrong"/"actually"/"no," after assistant output | 0.90 | `UserPreference` or `DebuggingLesson` |
//! | `ErrorResolution` | Tool failure → different-approach success | 0.85 | `ErrorResolution` |
//! | `RepeatedWorkflow` | Same tool sequence ≥3 times | 0.75 | `WorkflowPattern` |
//! | `ExplicitSave` | `/remember` command or `remember` tool | 1.00 | `UserPreference` |

use echo_core::memory::types::{MemorySource, MemoryType};
use futures::future::BoxFuture;
use std::collections::HashMap;

use super::security::InputTrustLevel;

// ── Trigger match ──────────────────────────────────────────────────────

/// A detected write trigger with associated memory data.
#[derive(Debug, Clone)]
pub struct TriggerMatch {
    /// The content to store as a memory.
    pub content: String,
    /// The memory type inferred from the trigger.
    pub memory_type: MemoryType,
    /// The source that triggered this write.
    pub source: MemorySource,
    /// Confidence from the source default.
    pub confidence: f32,
    /// Topic category inferred from context.
    pub topic: String,
    /// Trust level of the content source.
    pub trust_level: InputTrustLevel,
    /// A suggested key for storage.
    pub suggested_key: String,
    /// Exact source excerpts supporting this trigger.
    pub evidence: Vec<TriggerEvidence>,
}

/// One exact excerpt supporting a detected trigger.
#[derive(Debug, Clone)]
pub struct TriggerEvidence {
    /// Origin of the excerpt, such as `user`, `assistant`, or `tool_error`.
    pub source_role: String,
    /// Verbatim excerpt used by the detector.
    pub quote: String,
}

/// Result returned by an application-supplied trigger sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryTriggerDisposition {
    /// Continue with the framework's default durable-memory write.
    Persist,
    /// The sink captured the trigger and owns its subsequent lifecycle.
    Captured,
}

/// Optional application hook for routing detected triggers before persistence.
pub trait MemoryTriggerSink: Send + Sync {
    /// Route one detected trigger.
    fn on_trigger<'a>(
        &'a self,
        trigger: &'a TriggerMatch,
    ) -> BoxFuture<'a, std::result::Result<MemoryTriggerDisposition, String>>;
}

// ── Context records ────────────────────────────────────────────────────

/// Record of a tool failure.
#[derive(Debug, Clone)]
pub struct ToolFailureRecord {
    /// Tool name that failed.
    pub tool_name: String,
    /// Summary of the tool input.
    pub input_summary: String,
    /// Error message from the tool.
    pub error: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
}

/// Record of a successful tool use (after a previous failure).
#[derive(Debug, Clone)]
pub struct ToolSuccessRecord {
    /// Tool name that succeeded.
    pub tool_name: String,
    /// Summary of the tool input (may differ from the failure's input).
    pub input_summary: String,
    /// Summary of the tool output.
    pub output_summary: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
}

/// Record of an explicit save command.
#[derive(Debug, Clone)]
pub struct ExplicitSaveRecord {
    /// The content the user wants to remember.
    pub content: String,
    /// Optional topic hint from the user.
    pub topic: Option<String>,
}

/// Record of a tool invocation for sequence tracking.
#[derive(Debug, Clone)]
pub struct ToolSequenceRecord {
    /// Tool name that was invoked.
    pub tool_name: String,
    /// Session ID for cross-turn grouping.
    pub session_id: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
}

/// Detection context — the data needed to detect triggers.
#[derive(Debug, Clone, Default)]
pub struct TriggerContext {
    /// The user's message in the current turn.
    pub user_message: Option<String>,
    /// The assistant's last response (for correction detection).
    pub assistant_message: Option<String>,
    /// Most recent tool failure (for error resolution detection).
    pub last_tool_failure: Option<ToolFailureRecord>,
    /// Most recent tool success after a failure (for error resolution detection).
    pub last_tool_success: Option<ToolSuccessRecord>,
    /// Whether the user issued an explicit save command.
    pub explicit_save: Option<ExplicitSaveRecord>,
    /// Accumulated tool sequences for RepeatedWorkflow detection.
    pub tool_sequences: Vec<ToolSequenceRecord>,
}

// ── Correction signal patterns ─────────────────────────────────────────

/// Chinese correction signals — indicate the user is correcting the agent.
const CN_CORRECTION_SIGNALS: &[&str] = &[
    "不是这样",
    "不对",
    "不要",
    "不是",
    "不对的",
    "错了",
    "重新来",
    "别这样",
];

/// English correction signals — indicate the user is correcting the agent.
const EN_CORRECTION_SIGNALS: &[&str] = &[
    "wrong",
    "actually",
    "no,",
    "no ",
    "incorrect",
    "not like that",
    "that's wrong",
    "that's not right",
    "don't do that",
    "stop doing that",
    "not what i meant",
    "i meant",
    "let me clarify",
];

// ── TriggerDetector ────────────────────────────────────────────────────

/// Detects write triggers from conversation context.
///
/// All detection methods are synchronous (pattern matching only).
/// The caller is responsible for async persistence.
pub struct TriggerDetector {
    /// Maximum number of triggers to detect per turn.
    max_per_turn: usize,
}

impl TriggerDetector {
    /// Create a new trigger detector with default settings.
    pub fn new() -> Self {
        Self { max_per_turn: 5 }
    }

    /// Create a trigger detector with a custom max-per-turn limit.
    pub fn with_max_per_turn(max_per_turn: usize) -> Self {
        Self { max_per_turn }
    }

    /// Detect all triggers from the given context.
    ///
    /// Returns a list of `TriggerMatch` instances, each representing
    /// a memory that should be persisted. Capped at `max_per_turn`.
    pub fn detect(&self, ctx: &TriggerContext) -> Vec<TriggerMatch> {
        let mut matches = Vec::new();

        // Priority order: ExplicitSave > UserCorrection > ErrorResolution > RepeatedWorkflow
        if let Some(m) = self.detect_explicit_save(ctx) {
            matches.push(m);
        }
        if let Some(m) = self.detect_user_correction(ctx) {
            matches.push(m);
        }
        if let Some(m) = self.detect_error_resolution(ctx) {
            matches.push(m);
        }
        if let Some(m) = self.detect_repeated_workflow(ctx) {
            matches.push(m);
        }

        matches.truncate(self.max_per_turn);
        matches
    }

    /// Detect explicit save commands.
    ///
    /// Triggered by `/remember` command or `remember` tool invocation.
    fn detect_explicit_save(&self, ctx: &TriggerContext) -> Option<TriggerMatch> {
        let save = ctx.explicit_save.as_ref()?;
        let content = save.content.clone();
        let topic = save.topic.clone().unwrap_or_else(|| infer_topic(&content));
        let key = generate_key(MemorySource::ExplicitSave, &content);

        Some(TriggerMatch {
            content,
            memory_type: MemoryType::UserPreference,
            source: MemorySource::ExplicitSave,
            confidence: MemorySource::ExplicitSave.default_confidence(),
            topic,
            trust_level: InputTrustLevel::Trusted,
            suggested_key: key,
            evidence: vec![TriggerEvidence {
                source_role: "user".to_string(),
                quote: save.content.clone(),
            }],
        })
    }

    /// Detect user correction signals.
    ///
    /// Patterns: Chinese/English correction phrases that follow assistant output.
    fn detect_user_correction(&self, ctx: &TriggerContext) -> Option<TriggerMatch> {
        let user_msg = ctx.user_message.as_deref()?;
        let assistant_msg = ctx.assistant_message.as_deref()?;

        // Must have both messages and the user message must contain a correction signal
        let user_lower = user_msg.to_lowercase();
        let has_signal = CN_CORRECTION_SIGNALS.iter().any(|s| user_msg.contains(s))
            || EN_CORRECTION_SIGNALS.iter().any(|s| user_lower.contains(s));

        if !has_signal {
            return None;
        }

        let content = format!(
            "User correction: {} (was: {})",
            truncate_content(user_msg, 200),
            truncate_content(assistant_msg, 100)
        );
        let memory_type = classify_correction(user_msg, assistant_msg);
        let topic = infer_topic(user_msg);
        let key = generate_key(MemorySource::UserCorrection, &content);

        Some(TriggerMatch {
            content,
            memory_type,
            source: MemorySource::UserCorrection,
            confidence: MemorySource::UserCorrection.default_confidence(),
            topic,
            trust_level: InputTrustLevel::Trusted,
            suggested_key: key,
            evidence: vec![
                TriggerEvidence {
                    source_role: "user".to_string(),
                    quote: truncate_content(user_msg, 200),
                },
                TriggerEvidence {
                    source_role: "assistant".to_string(),
                    quote: truncate_content(assistant_msg, 100),
                },
            ],
        })
    }

    /// Detect error resolution patterns.
    ///
    /// Pattern: tool failure followed by success with a different approach.
    fn detect_error_resolution(&self, ctx: &TriggerContext) -> Option<TriggerMatch> {
        let failure = ctx.last_tool_failure.as_ref()?;
        let success = ctx.last_tool_success.as_ref()?;

        // The success must come after the failure and use a different approach
        // (different tool or different input)
        let different_approach = success.tool_name != failure.tool_name
            || success.input_summary != failure.input_summary;

        if !different_approach {
            return None;
        }

        let content = format!(
            "Error resolved: {} failed ({}) → {} succeeded ({})",
            failure.tool_name,
            truncate_content(&failure.error, 80),
            success.tool_name,
            truncate_content(&success.output_summary, 80)
        );
        let topic = infer_topic(&failure.error);
        let key = generate_key(MemorySource::ErrorResolution, &content);

        Some(TriggerMatch {
            content,
            memory_type: MemoryType::ErrorResolution,
            source: MemorySource::ErrorResolution,
            confidence: MemorySource::ErrorResolution.default_confidence(),
            topic,
            trust_level: InputTrustLevel::Assistant,
            suggested_key: key,
            evidence: vec![
                TriggerEvidence {
                    source_role: "tool_error".to_string(),
                    quote: format!(
                        "{}: {}",
                        failure.tool_name,
                        truncate_content(&failure.error, 160)
                    ),
                },
                TriggerEvidence {
                    source_role: "tool_output".to_string(),
                    quote: format!(
                        "{}: {}",
                        success.tool_name,
                        truncate_content(&success.output_summary, 160)
                    ),
                },
            ],
        })
    }

    /// Detect repeated workflow patterns.
    ///
    /// Pattern: same tool sequence observed ≥3 times across sessions.
    fn detect_repeated_workflow(&self, ctx: &TriggerContext) -> Option<TriggerMatch> {
        if ctx.tool_sequences.len() < 3 {
            return None;
        }

        // Group sequences by (tool_name, session_id) to find repeated patterns
        let mut tool_counts: HashMap<String, usize> = HashMap::new();
        for seq in &ctx.tool_sequences {
            *tool_counts.entry(seq.tool_name.clone()).or_insert(0) += 1;
        }

        // Find the most repeated tool sequence (≥3 occurrences)
        let most_repeated = tool_counts
            .iter()
            .filter(|&(_, &count)| count >= 3)
            .max_by_key(|&(_, &count)| count)?;

        let tool_name = most_repeated.0.clone();
        let count = *most_repeated.1;

        let content = format!(
            "Repeated workflow pattern: tool '{}' used {} times across sessions",
            tool_name, count
        );
        let topic = infer_topic(&tool_name);
        let key = generate_key(MemorySource::RepeatedWorkflow, &content);

        Some(TriggerMatch {
            content,
            memory_type: MemoryType::WorkflowPattern,
            source: MemorySource::RepeatedWorkflow,
            confidence: MemorySource::RepeatedWorkflow.default_confidence(),
            topic,
            trust_level: InputTrustLevel::Assistant,
            suggested_key: key,
            evidence: ctx
                .tool_sequences
                .iter()
                .filter(|record| record.tool_name == tool_name)
                .take(3)
                .map(|record| TriggerEvidence {
                    source_role: "tool_sequence".to_string(),
                    quote: format!(
                        "{} in session {} at {}",
                        record.tool_name, record.session_id, record.timestamp
                    ),
                })
                .collect(),
        })
    }
}

impl Default for TriggerDetector {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helper functions ───────────────────────────────────────────────────

/// Classify a user correction into a MemoryType based on content signals.
fn classify_correction(user_msg: &str, _assistant_msg: &str) -> MemoryType {
    let lower = user_msg.to_lowercase();

    // Debugging/error correction
    if lower.contains("bug")
        || lower.contains("error")
        || lower.contains("fix")
        || lower.contains("debug")
        || lower.contains("fail")
        || lower.contains("错误")
        || lower.contains("修复")
        || lower.contains("调试")
    {
        return MemoryType::DebuggingLesson;
    }

    // Style/preference correction
    if lower.contains("prefer")
        || lower.contains("style")
        || lower.contains("format")
        || lower.contains("verbose")
        || lower.contains("concise")
        || lower.contains("always")
        || lower.contains("never")
        || lower.contains("风格")
        || lower.contains("格式")
        || lower.contains("简洁")
    {
        return MemoryType::UserPreference;
    }

    // Tool/command correction
    if lower.contains("tool")
        || lower.contains("command")
        || lower.contains("utility")
        || lower.contains("工具")
        || lower.contains("命令")
    {
        return MemoryType::ToolUsage;
    }

    // Default: user preference (the correction itself is a preference signal)
    MemoryType::UserPreference
}

/// Infer a topic from content using keyword heuristics.
fn infer_topic(content: &str) -> String {
    let lower = content.to_lowercase();

    if lower.contains("build")
        || lower.contains("compile")
        || lower.contains("maven")
        || lower.contains("cargo")
    {
        return "build".to_string();
    }
    if lower.contains("test") || lower.contains("spec") || lower.contains("验证") {
        return "testing".to_string();
    }
    if lower.contains("deploy") || lower.contains("release") || lower.contains("发布") {
        return "deploy".to_string();
    }
    if lower.contains("debug")
        || lower.contains("error")
        || lower.contains("fix")
        || lower.contains("bug")
        || lower.contains("错误")
    {
        return "debugging".to_string();
    }
    if lower.contains("style")
        || lower.contains("format")
        || lower.contains("prefer")
        || lower.contains("风格")
    {
        return "style".to_string();
    }
    if lower.contains("arch")
        || lower.contains("design")
        || lower.contains("架构")
        || lower.contains("设计")
    {
        return "architecture".to_string();
    }
    if lower.contains("tool") || lower.contains("command") || lower.contains("工具") {
        return "tools".to_string();
    }
    if lower.contains("git") || lower.contains("branch") || lower.contains("commit") {
        return "git".to_string();
    }

    "general".to_string()
}

/// Generate a deterministic key from source and content.
fn generate_key(source: MemorySource, content: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    let hash = hasher.finish();
    let prefix = match source {
        MemorySource::UserCorrection => "uc",
        MemorySource::ErrorResolution => "er",
        MemorySource::RepeatedWorkflow => "rw",
        MemorySource::ExplicitSave => "es",
        MemorySource::AutoExtracted => "ae",
        MemorySource::L3Promotion => "l3",
    };
    format!("{prefix}_{hash:016x}")
}

/// Truncate content to a maximum length, appending "..." if truncated.
fn truncate_content(s: &str, max_len: usize) -> String {
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(max_len).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_user_correction_chinese() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            user_message: Some("不是这样做的，应该用 cargo test".to_string()),
            assistant_message: Some("Run the tests with make test".to_string()),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert_eq!(matches.len(), 1);
        let Some(detected) = matches.first() else {
            return;
        };
        assert_eq!(detected.source, MemorySource::UserCorrection);
        assert_eq!(detected.confidence, 0.90);
        assert_eq!(detected.evidence.len(), 2);
        assert!(
            detected
                .evidence
                .iter()
                .any(|item| item.source_role == "user")
        );
    }

    #[test]
    fn truncate_content_counts_unicode_characters() {
        assert_eq!(truncate_content("中文🙂abc", 3), "中文🙂...");
        assert_eq!(truncate_content("中文", 3), "中文");
    }

    #[test]
    fn test_detect_user_correction_english() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            user_message: Some("Actually, that's wrong — use cargo test instead".to_string()),
            assistant_message: Some("Run the tests with make test".to_string()),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            matches
                .iter()
                .any(|m| m.source == MemorySource::UserCorrection)
        );
    }

    #[test]
    fn test_no_false_positive_correction() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            user_message: Some("Please add a test for the new feature".to_string()),
            assistant_message: Some("I'll add a test now".to_string()),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            !matches
                .iter()
                .any(|m| m.source == MemorySource::UserCorrection)
        );
    }

    #[test]
    fn test_no_correction_without_assistant_message() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            user_message: Some("不对，应该这样做".to_string()),
            assistant_message: None,
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            !matches
                .iter()
                .any(|m| m.source == MemorySource::UserCorrection)
        );
    }

    #[test]
    fn test_detect_error_resolution() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            last_tool_failure: Some(ToolFailureRecord {
                tool_name: "Bash".to_string(),
                input_summary: "make test".to_string(),
                error: "make: *** No rule to make target 'test'".to_string(),
                timestamp: "2026-06-15T10:00:00Z".to_string(),
            }),
            last_tool_success: Some(ToolSuccessRecord {
                tool_name: "Bash".to_string(),
                input_summary: "cargo test".to_string(),
                output_summary: "running 42 tests ... ok".to_string(),
                timestamp: "2026-06-15T10:01:00Z".to_string(),
            }),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            matches
                .iter()
                .any(|m| m.source == MemorySource::ErrorResolution)
        );
        let er = matches
            .iter()
            .find(|m| m.source == MemorySource::ErrorResolution)
            .unwrap();
        assert_eq!(er.memory_type, MemoryType::ErrorResolution);
        assert_eq!(er.confidence, 0.85);
    }

    #[test]
    fn test_no_error_resolution_same_approach() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            last_tool_failure: Some(ToolFailureRecord {
                tool_name: "Bash".to_string(),
                input_summary: "cargo test".to_string(),
                error: "test failed".to_string(),
                timestamp: "2026-06-15T10:00:00Z".to_string(),
            }),
            last_tool_success: Some(ToolSuccessRecord {
                tool_name: "Bash".to_string(),
                input_summary: "cargo test".to_string(), // same approach
                output_summary: "all tests passed".to_string(),
                timestamp: "2026-06-15T10:01:00Z".to_string(),
            }),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            !matches
                .iter()
                .any(|m| m.source == MemorySource::ErrorResolution)
        );
    }

    #[test]
    fn test_detect_explicit_save() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            explicit_save: Some(ExplicitSaveRecord {
                content: "This project uses Java 8 with Maven".to_string(),
                topic: Some("build".to_string()),
            }),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            matches
                .iter()
                .any(|m| m.source == MemorySource::ExplicitSave)
        );
        let es = matches
            .iter()
            .find(|m| m.source == MemorySource::ExplicitSave)
            .unwrap();
        assert_eq!(es.memory_type, MemoryType::UserPreference);
        assert_eq!(es.confidence, 1.0);
        assert_eq!(es.topic, "build");
    }

    #[test]
    fn test_detect_repeated_workflow() {
        let detector = TriggerDetector::new();
        let sequences: Vec<ToolSequenceRecord> = (0..4)
            .map(|i| ToolSequenceRecord {
                tool_name: "search_paper".to_string(),
                session_id: format!("session-{i}"),
                timestamp: format!("2026-06-15T10:{i:02}:00Z"),
            })
            .collect();
        let ctx = TriggerContext {
            tool_sequences: sequences,
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            matches
                .iter()
                .any(|m| m.source == MemorySource::RepeatedWorkflow)
        );
        let rw = matches
            .iter()
            .find(|m| m.source == MemorySource::RepeatedWorkflow)
            .unwrap();
        assert_eq!(rw.memory_type, MemoryType::WorkflowPattern);
        assert_eq!(rw.confidence, 0.75);
    }

    #[test]
    fn test_no_repeated_workflow_under_threshold() {
        let detector = TriggerDetector::new();
        let ctx = TriggerContext {
            tool_sequences: vec![
                ToolSequenceRecord {
                    tool_name: "search_paper".to_string(),
                    session_id: "s1".to_string(),
                    timestamp: "2026-06-15T10:00:00Z".to_string(),
                },
                ToolSequenceRecord {
                    tool_name: "search_paper".to_string(),
                    session_id: "s2".to_string(),
                    timestamp: "2026-06-15T10:01:00Z".to_string(),
                },
            ],
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(
            !matches
                .iter()
                .any(|m| m.source == MemorySource::RepeatedWorkflow)
        );
    }

    #[test]
    fn test_max_per_turn_limit() {
        let detector = TriggerDetector::with_max_per_turn(1);
        let ctx = TriggerContext {
            user_message: Some("不对，实际上应该用 cargo build".to_string()),
            assistant_message: Some("Run make build".to_string()),
            explicit_save: Some(ExplicitSaveRecord {
                content: "Use cargo build".to_string(),
                topic: None,
            }),
            ..Default::default()
        };
        let matches = detector.detect(&ctx);
        assert!(matches.len() <= 1);
    }

    #[test]
    fn test_classify_correction_debugging() {
        assert_eq!(
            classify_correction("不对，这个 bug 应该用别的方式修复", ""),
            MemoryType::DebuggingLesson
        );
        assert_eq!(
            classify_correction("Wrong, that error needs a different fix", ""),
            MemoryType::DebuggingLesson
        );
    }

    #[test]
    fn test_classify_correction_style() {
        assert_eq!(
            classify_correction("不要这样，我更喜欢简洁的代码风格", ""),
            MemoryType::UserPreference
        );
        assert_eq!(
            classify_correction("Actually, I prefer concise format", ""),
            MemoryType::UserPreference
        );
    }

    #[test]
    fn test_generate_key_deterministic() {
        let key1 = generate_key(MemorySource::UserCorrection, "same content");
        let key2 = generate_key(MemorySource::UserCorrection, "same content");
        assert_eq!(key1, key2);

        let key3 = generate_key(MemorySource::ErrorResolution, "same content");
        assert_ne!(key1, key3); // different prefix
    }

    #[test]
    fn test_infer_topic() {
        assert_eq!(infer_topic("cargo build failed"), "build");
        assert_eq!(infer_topic("test error in module"), "testing");
        assert_eq!(infer_topic("deploy to production"), "deploy");
        assert_eq!(infer_topic("debug the error"), "debugging");
        assert_eq!(infer_topic("I prefer this style"), "style");
        assert_eq!(infer_topic("architecture decision"), "architecture");
        assert_eq!(infer_topic("use the grep tool"), "tools");
        assert_eq!(infer_topic("git commit the changes"), "git");
        assert_eq!(infer_topic("something random"), "general");
    }

    #[test]
    fn test_truncate_content_short() {
        assert_eq!(truncate_content("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let long = "a".repeat(300);
        let truncated = truncate_content(&long, 200);
        assert!(truncated.len() <= 203); // 200 + "..."
        assert!(truncated.ends_with("..."));
    }
}
