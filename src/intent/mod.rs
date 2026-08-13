//! Intent Router — lightweight intent classification before ReAct loop entry.
//!
//! Provides a decision point at the start of every user turn:
//!
//! | Intent variant      | Action                                        |
//! |---------------------|-----------------------------------------------|
//! | `DirectAnswer`      | Skip ReAct, call LLM directly                 |
//! | `SkillRequired`     | Activate skill, then run ReAct                |
//! | `Fallback`          | Proceed with normal ReAct loop                |
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use echo_agent::prelude::*;
//! use echo_agent::intent::{IntentRouter, Intent};
//!
//! let router = IntentRouter::new(
//!     Box::new(MyClassifier::default()),
//!     IntentRouterConfig::default(),
//! );
//!
//! let agent = ReactAgentBuilder::new()
//!     .intent_router(router)
//!     .build()?;
//! ```

use crate::llm::types::Message;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

// ── Intent ───────────────────────────────────────────────────────────

/// Classification result produced by an [`IntentClassifier`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Intent {
    /// Simple question — answer directly without entering the ReAct loop.
    DirectAnswer { confidence: f32 },
    /// Activate a skill by name before entering the ReAct loop.
    SkillRequired { skill_name: String, confidence: f32 },
    /// Confidence too low — fall back to the normal ReAct loop.
    Fallback,
}

impl Intent {
    /// Return the confidence level, if any.
    pub fn confidence(&self) -> Option<f32> {
        match self {
            Intent::DirectAnswer { confidence } => Some(*confidence),
            Intent::SkillRequired { confidence, .. } => Some(*confidence),
            Intent::Fallback => None,
        }
    }

    /// Return the skill name, if this is a `SkillRequired` intent.
    pub fn skill_name(&self) -> Option<&str> {
        match self {
            Intent::SkillRequired { skill_name, .. } => Some(skill_name.as_str()),
            _ => None,
        }
    }
}

// ── IntentClassifier ───────────────────────────────────────────────────

/// Trait for pluggable intent classification strategies.
///
/// Implementations may use keyword matching, LLM-based classification,
/// embedding similarity, or any combination thereof.
pub trait IntentClassifier: Send + Sync {
    /// Classify the user's input given optional conversation context.
    fn classify<'a>(&'a self, user_input: &'a str, context: &'a [Message])
    -> BoxFuture<'a, Intent>;
}

// ── IntentRouterConfig ────────────────────────────────────────────────

/// Configuration for the [`IntentRouter`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentRouterConfig {
    /// Minimum confidence to trust a classification result.
    /// Values below this threshold are treated as [`Intent::Fallback`].
    pub confidence_threshold: f32,
    /// Whether the `DirectAnswer` shortcut is enabled.
    pub enable_direct_answer: bool,
    /// Whether the `SkillRequired` shortcut is enabled.
    pub enable_skill_routing: bool,
    /// Maximum time spent in pre-routing classification.
    #[serde(default = "default_classification_timeout_ms")]
    pub classification_timeout_ms: u64,
}

fn default_classification_timeout_ms() -> u64 {
    5_000
}

impl Default for IntentRouterConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.7,
            enable_direct_answer: true,
            enable_skill_routing: true,
            classification_timeout_ms: default_classification_timeout_ms(),
        }
    }
}

// ── IntentRouter ──────────────────────────────────────────────────────

/// Central routing component placed at the ReAct loop entry.
///
/// When attached to a [`ReactAgent`](crate::agent::ReactAgent), every user
/// message is first classified by the configured [`IntentClassifier`];
/// depending on the returned [`Intent`], the agent either shortcuts to a
/// direct answer, activates a skill, or proceeds with the
/// standard ReAct loop.
#[derive(Clone)]
pub struct IntentRouter {
    classifier: std::sync::Arc<dyn IntentClassifier>,
    config: IntentRouterConfig,
    available_skills: Option<std::sync::Arc<HashSet<String>>>,
}

impl IntentRouter {
    /// Create a new router from a classifier and configuration.
    pub fn new(classifier: Box<dyn IntentClassifier>, config: IntentRouterConfig) -> Self {
        Self {
            classifier: std::sync::Arc::from(classifier),
            config,
            available_skills: None,
        }
    }

    /// Fence executable skill decisions to the catalog used by the caller.
    pub fn with_available_skills(mut self, skills: impl IntoIterator<Item = String>) -> Self {
        self.available_skills = Some(std::sync::Arc::new(skills.into_iter().collect()));
        self
    }

    /// Classify user input and return the routing decision.
    pub async fn classify(&self, user_input: &str, context: &[Message]) -> Intent {
        self.classify_with_cancel(user_input, context, CancellationToken::new())
            .await
    }

    /// Classify within the invocation cancellation domain and configured deadline.
    pub async fn classify_with_cancel(
        &self,
        user_input: &str,
        context: &[Message],
        cancel: CancellationToken,
    ) -> Intent {
        let timeout = Duration::from_millis(self.config.classification_timeout_ms.max(1));
        let raw = tokio::select! {
            _ = cancel.cancelled() => return Intent::Fallback,
            result = tokio::time::timeout(timeout, self.classifier.classify(user_input, context)) => {
                match result {
                    Ok(intent) => intent,
                    Err(_) => return Intent::Fallback,
                }
            }
        };
        match raw {
            Intent::DirectAnswer { confidence } => {
                if self.config.enable_direct_answer
                    && confidence >= self.config.confidence_threshold
                {
                    Intent::DirectAnswer { confidence }
                } else {
                    Intent::Fallback
                }
            }
            Intent::SkillRequired {
                skill_name,
                confidence,
            } => {
                if self.config.enable_skill_routing
                    && confidence >= self.config.confidence_threshold
                    && self
                        .available_skills
                        .as_ref()
                        .is_none_or(|skills| skills.contains(&skill_name))
                {
                    Intent::SkillRequired {
                        skill_name,
                        confidence,
                    }
                } else {
                    Intent::Fallback
                }
            }
            Intent::Fallback => Intent::Fallback,
        }
    }
}

// ── Re-export convenience types ────────────────────────────────────────

pub use self::classifier::{
    ChainedClassifier, KeywordClassifier, KeywordClassifierConfig, LlmIntentClassifier,
    SkillDescription,
};
pub use self::trigger_supervisor::TriggerSupervisor;

mod classifier;
mod trigger_supervisor;
