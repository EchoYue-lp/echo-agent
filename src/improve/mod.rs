//! Evolution helpers and optional eval-driven improvement.
//!
//! # Overview
//!
//! The base `improve` feature provides explicit trajectory export plus compatibility
//! re-exports for evolution types. Eval-driven analysis is available only when the
//! `eval` feature is enabled as well.
//!
//! # Safety
//!
//! All suggestions require human review. This module does NOT automatically:
//! - Modify core runtime code
//! - Relax security policies
//! - Change permission rules
//! - Publish or deploy anything
//!
//! # Components
//!
//! - [`TrajectorySaver`] — Optional ShareGPT JSONL export for framework consumers
//!   that explicitly build fine-tuning datasets.
//! - [`BackgroundReviewer`] and [`Curator`] — Compatibility re-exports from
//!   [`evolution`](crate::evolution). Product integrations should use that module directly.
//! ## Eval-Driven Improvement (`improve` + `eval`)
//!
//! - `Analyzer` — Statically analyzes Run traces to detect failure patterns.
//! - `ImprovementLoop` — Iterative prompt optimization loop.
//! - `EvalDrivenImprovement` — Unified entry point wrapping ImprovementLoop with HTML reports.
//! - `PromptGenerator` — LLM-driven prompt improvement generator.
//!
//! # Complementary Module: `evolution`
//!
//! The [`evolution`](crate::evolution) module provides the memory/skill/rule
//! lifecycle management system with typed metadata, change audit, and security.
//! It complements this module's eval-driven improvement approach.
//!
//! # Data Storage Locations
//!
//! - Trajectories: `~/.echo-agent/trajectories/YYYY-MM-DD.jsonl`
//! - Curator state: `~/.echo-agent/curator_state.json`
//! - Background review candidates: returned to the caller; optional user-preference
//!   persistence uses the configured evolution memory layer

pub mod background_review;
pub mod trajectory;

#[cfg(feature = "eval")]
pub mod analyzer;
#[cfg(feature = "eval")]
pub mod eval_improvement;
#[cfg(feature = "eval")]
pub mod generator;
#[cfg(feature = "eval")]
pub mod r#loop;

// Re-export Curator types from evolution module
pub use crate::evolution::curator::{
    Curator, CuratorConfig, CuratorState, CuratorStatus, SkillLifecycle,
};
pub use background_review::{
    BackgroundReviewConfig, BackgroundReviewer, ReviewCandidate, ReviewCandidateKind, ReviewOutcome,
};
pub use trajectory::{TrajectoryEntry, TrajectorySaver, TrajectoryStats};

#[cfg(feature = "eval")]
pub use analyzer::Analyzer;
#[cfg(feature = "eval")]
pub use eval_improvement::EvalDrivenImprovement;
#[cfg(feature = "eval")]
pub use generator::PromptGenerator;
#[cfg(feature = "eval")]
pub use r#loop::{ImprovementLoop, LoopResult};

#[cfg(feature = "eval")]
use serde::{Deserialize, Serialize};

// ── CritiqueIssue ────────────────────────────────────────────────────

/// A specific issue found in a run trace.
#[cfg(feature = "eval")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CritiqueIssue {
    /// Agent wrote to a file without reading it first.
    WriteWithoutRead { tool: String, count: usize },
    /// Tool was retried excessively.
    ExcessiveRetries { tool: String, count: usize },
    /// A tool error pattern (repeated failures of the same tool).
    ToolErrorPattern { tool: String, message: String },
    /// Context overflow — compression was triggered.
    ContextOverflow {
        tokens_before: usize,
        tokens_after: usize,
    },
    /// The agent didn't use a tool that seemed necessary.
    MissingTool { needed: String },
    /// Too many tool calls for a simple task.
    ExcessiveToolCalls { total: usize },
}

// ── ImprovementSuggestion ────────────────────────────────────────────

/// A concrete, human-reviewable improvement suggestion.
#[cfg(feature = "eval")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImprovementSuggestion {
    /// Suggest changing the system prompt.
    PromptChange {
        /// Which section to modify (e.g. "tools", "behavior", "constraints").
        section: String,
        /// What to add or change.
        suggestion: String,
    },
    /// Suggest a new or changed policy rule.
    PolicyChange {
        /// The new rule description.
        rule: String,
        /// Why this rule is needed (based on observed failures).
        reason: String,
    },
    /// Generate a new eval case from this run.
    EvalGeneration {
        /// Proposed case ID.
        case_id: String,
        /// The eval case content (JSON).
        json: String,
    },
}

// ── RunCritique ──────────────────────────────────────────────────────

/// Complete analysis of a single run.
#[cfg(feature = "eval")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCritique {
    /// The run being analyzed.
    pub run_id: String,
    /// Whether the run was successful overall.
    pub success: bool,
    /// Total score (0.0 - 1.0).
    pub score: f64,
    /// Issues found.
    pub issues: Vec<CritiqueIssue>,
    /// Improvement suggestions.
    pub suggestions: Vec<ImprovementSuggestion>,
}

#[cfg(feature = "eval")]
impl RunCritique {
    /// Create an empty critique for a run.
    pub fn new(run_id: &str, success: bool) -> Self {
        Self {
            run_id: run_id.to_string(),
            success,
            score: if success { 1.0 } else { 0.0 },
            issues: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Add an issue.
    pub fn with_issue(mut self, issue: CritiqueIssue) -> Self {
        self.issues.push(issue);
        self
    }

    /// Add a suggestion.
    pub fn with_suggestion(mut self, suggestion: ImprovementSuggestion) -> Self {
        self.suggestions.push(suggestion);
        self
    }

    /// Format as a human-readable report.
    pub fn format_report(&self) -> String {
        let mut lines = vec![
            format!("Run: {}", self.run_id),
            format!("Success: {} (score: {:.2})", self.success, self.score),
            String::new(),
        ];

        if !self.issues.is_empty() {
            lines.push("Issues Found:".into());
            for issue in &self.issues {
                lines.push(match issue {
                    CritiqueIssue::WriteWithoutRead { tool, count } => {
                        format!("  - Write without read: {tool} was called {count} time(s) without prior read_file")
                    }
                    CritiqueIssue::ExcessiveRetries { tool, count } => {
                        format!("  - Excessive retries: {tool} was retried {count} time(s)")
                    }
                    CritiqueIssue::ToolErrorPattern { tool, message } => {
                        format!("  - Tool error: {tool} — {message}")
                    }
                    CritiqueIssue::ContextOverflow { tokens_before, tokens_after } => {
                        format!("  - Context overflow: compressed from {tokens_before} to {tokens_after} tokens")
                    }
                    CritiqueIssue::MissingTool { needed } => {
                        format!("  - Missing tool: {needed} might have helped")
                    }
                    CritiqueIssue::ExcessiveToolCalls { total } => {
                        format!("  - Excessive tool calls: {total} total")
                    }
                });
            }
            lines.push(String::new());
        }

        if !self.suggestions.is_empty() {
            lines.push("Suggestions:".into());
            for suggestion in &self.suggestions {
                match suggestion {
                    ImprovementSuggestion::PromptChange {
                        section,
                        suggestion,
                    } => {
                        lines.push(format!("  [Prompt] {section}: {suggestion}"));
                    }
                    ImprovementSuggestion::PolicyChange { rule, reason } => {
                        lines.push(format!("  [Policy] {rule} — Reason: {reason}"));
                    }
                    ImprovementSuggestion::EvalGeneration { case_id, .. } => {
                        lines.push(format!("  [Eval] Generate case: {case_id}"));
                    }
                }
            }
        }

        if self.issues.is_empty() && self.suggestions.is_empty() {
            lines.push("No issues or suggestions. The run looks clean.".into());
        }

        lines.join("\n")
    }
}
