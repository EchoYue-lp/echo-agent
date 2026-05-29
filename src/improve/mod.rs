//! Self-improvement pipeline — analyze failures and suggest improvements.
//!
//! # Overview
//!
//! The improvement system analyzes completed [`Run`](crate::trace::Run) traces
//! to detect failure patterns and generate human-reviewable improvement suggestions.
//!
//! # Safety
//!
//! All suggestions require human review. This module does NOT automatically:
//! - Modify core runtime code
//! - Relax security policies
//! - Change permission rules
//! - Publish or deploy anything

pub mod analyzer;
pub mod background_review;
pub mod curator;
pub mod evolution;
pub mod generator;
pub mod r#loop;
pub mod store;
pub mod trajectory;
pub use analyzer::Analyzer;
pub use background_review::{BackgroundReviewConfig, BackgroundReviewer, ReviewOutcome};
pub use curator::{Curator, CuratorConfig, CuratorState, CuratorStatus, SkillLifecycle};
pub use evolution::SelfEvolution;
pub use generator::PromptGenerator;
pub use r#loop::{ImprovementLoop, LoopResult};
pub use store::CritiqueStore;
pub use trajectory::{TrajectoryEntry, TrajectorySaver, TrajectoryStats};

use serde::{Deserialize, Serialize};

// ── CritiqueIssue ────────────────────────────────────────────────────

/// A specific issue found in a run trace.
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
