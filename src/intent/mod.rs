//! Intent Router — lightweight intent classification before ReAct loop entry.
//!
//! Provides a decision point at the start of every user turn:
//!
//! | Intent variant      | Action                                        |
//! |---------------------|-----------------------------------------------|
//! | `DirectAnswer`      | Skip ReAct, call LLM directly                 |
//! | `SkillRequired`     | Activate skill, then run ReAct                |
//! | `WorkflowRequired`  | Execute workflow graph, then run ReAct        |
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

// ── Intent ───────────────────────────────────────────────────────────

/// Classification result produced by an [`IntentClassifier`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Intent {
    /// Simple question — answer directly without entering the ReAct loop.
    DirectAnswer { confidence: f32 },
    /// Activate a skill by name before entering the ReAct loop.
    SkillRequired { skill_name: String, confidence: f32 },
    /// Execute a named workflow graph before entering the ReAct loop.
    WorkflowRequired {
        workflow_name: String,
        confidence: f32,
    },
    /// Confidence too low — fall back to the normal ReAct loop.
    Fallback,
}

impl Intent {
    /// Return the confidence level, if any.
    pub fn confidence(&self) -> Option<f32> {
        match self {
            Intent::DirectAnswer { confidence } => Some(*confidence),
            Intent::SkillRequired { confidence, .. } => Some(*confidence),
            Intent::WorkflowRequired { confidence, .. } => Some(*confidence),
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

    /// Return the workflow name, if this is a `WorkflowRequired` intent.
    pub fn workflow_name(&self) -> Option<&str> {
        match self {
            Intent::WorkflowRequired { workflow_name, .. } => Some(workflow_name.as_str()),
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
    /// Whether the `WorkflowRequired` shortcut is enabled.
    pub enable_workflow_routing: bool,
}

impl Default for IntentRouterConfig {
    fn default() -> Self {
        Self {
            confidence_threshold: 0.7,
            enable_direct_answer: true,
            enable_skill_routing: true,
            enable_workflow_routing: true,
        }
    }
}

// ── IntentRouter ──────────────────────────────────────────────────────

/// Central routing component placed at the ReAct loop entry.
///
/// When attached to a [`ReactAgent`](crate::agent::ReactAgent), every user
/// message is first classified by the configured [`IntentClassifier`];
/// depending on the returned [`Intent`], the agent either shortcuts to a
/// direct answer, activates a skill, runs a workflow, or proceeds with the
/// standard ReAct loop.
#[derive(Clone)]
pub struct IntentRouter {
    classifier: std::sync::Arc<dyn IntentClassifier>,
    config: IntentRouterConfig,
}

impl IntentRouter {
    /// Create a new router from a classifier and configuration.
    pub fn new(classifier: Box<dyn IntentClassifier>, config: IntentRouterConfig) -> Self {
        Self {
            classifier: std::sync::Arc::from(classifier),
            config,
        }
    }

    /// Classify user input and return the routing decision.
    pub async fn classify(&self, user_input: &str, context: &[Message]) -> Intent {
        let raw = self.classifier.classify(user_input, context).await;
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
                {
                    Intent::SkillRequired {
                        skill_name,
                        confidence,
                    }
                } else {
                    Intent::Fallback
                }
            }
            Intent::WorkflowRequired {
                workflow_name,
                confidence,
            } => {
                if self.config.enable_workflow_routing
                    && confidence >= self.config.confidence_threshold
                {
                    Intent::WorkflowRequired {
                        workflow_name,
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
