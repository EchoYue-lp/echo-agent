//! Critic module — evaluation and feedback for agent outputs
//!
//! Provides tools and utilities for evaluating agent outputs and providing
//! structured feedback. Aligns with industry patterns where reflection is
//! a composable capability rather than a separate agent type.

mod llm_critic;
mod review_tool;

pub use llm_critic::LlmCritic;
pub use review_tool::ReviewTool;

// Re-export core critic types for convenience
pub use echo_core::agent::{
    CompositeCritic, CompositeStrategy, Critic, StaticCritic, ThresholdCritic,
};
pub use echo_core::agent::{Critique, CritiqueOutput, critique_output_schema};
