//! Self-Reflection type definitions

use serde::{Deserialize, Serialize};

/// Evaluation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Critique {
    /// Quality score (0.0 - 10.0)
    pub score: f64,
    /// Whether the quality threshold was passed
    pub passed: bool,
    /// Detailed feedback
    pub feedback: String,
    /// Improvement suggestions
    #[serde(default)]
    pub suggestions: Vec<String>,
}

/// Structured output for evaluation results (for LLM JSON parsing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CritiqueOutput {
    /// Quality score (0.0 - 10.0)
    pub score: f64,
    /// Whether the quality threshold was passed
    pub passed: bool,
    /// Detailed feedback
    pub feedback: String,
    /// Improvement suggestions
    #[serde(default)]
    pub suggestions: Vec<String>,
}

impl From<CritiqueOutput> for Critique {
    fn from(output: CritiqueOutput) -> Self {
        Self {
            score: output.score,
            passed: output.passed,
            feedback: output.feedback,
            suggestions: output.suggestions,
        }
    }
}

/// Record of a single reflection iteration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionRecord {
    /// Iteration number (0-based)
    pub iteration: usize,
    /// Current response
    pub answer: String,
    /// Evaluation result
    pub critique: Critique,
    /// Reflection text (analysis of failure reasons)
    pub reflection_text: String,
    /// Refined response
    pub refined_answer: Option<String>,
}

/// Reflection experience (for episodic memory)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionExperience {
    /// Unique ID
    pub id: String,
    /// Lesson learned (e.g., "Confirm data availability before querying")
    pub lesson: String,
    /// Error pattern (e.g., "Assumes data exists without checking")
    pub error_pattern: String,
    /// Task category
    #[serde(default)]
    pub task_category: Option<String>,
    /// Number of times referenced
    #[serde(default)]
    pub use_count: u32,
}

impl ReflectionExperience {
    /// Create a reflection experience record
    ///
    /// # Parameters
    /// * `lesson` - Lesson learned from errors (positive formulation)
    /// * `error_pattern` - Observed error pattern (negative formulation)
    ///
    /// # Description
    /// Automatically generates a unique ID, initial use count is 0, task category is empty.
    pub fn new(lesson: impl Into<String>, error_pattern: impl Into<String>) -> Self {
        Self {
            id: format!("exp_{}", uuid::Uuid::new_v4().as_simple()),
            lesson: lesson.into(),
            error_pattern: error_pattern.into(),
            task_category: None,
            use_count: 0,
        }
    }

    /// Set task category
    ///
    /// # Parameters
    /// * `category` - Task category identifier, used for classification filtering during experience retrieval
    ///
    /// # Description
    /// Once set, this experience can be filtered by category during subsequent retrieval, improving relevance.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.task_category = Some(category.into());
        self
    }
}

/// Refinement prompt builder trait
pub trait RefinementPromptBuilder: Send + Sync {
    /// Build refinement prompt
    ///
    /// - `task`: Original task
    /// - `current_answer`: Current response
    /// - `critique`: Evaluation result
    /// - `reflection`: Reflection text
    /// - `iteration`: Current iteration count
    fn build_prompt(
        &self,
        task: &str,
        current_answer: &str,
        critique: &Critique,
        reflection: &str,
        iteration: usize,
    ) -> String;
}

/// Default refinement prompt builder
pub struct DefaultRefinementPromptBuilder;

impl RefinementPromptBuilder for DefaultRefinementPromptBuilder {
    fn build_prompt(
        &self,
        task: &str,
        current_answer: &str,
        critique: &Critique,
        reflection: &str,
        iteration: usize,
    ) -> String {
        let suggestions_text = if critique.suggestions.is_empty() {
            String::new()
        } else {
            format!(
                "\nImprovement suggestions:\n{}",
                critique
                    .suggestions
                    .iter()
                    .map(|s| format!("- {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        format!(
            "Original task: {}\n\n\
             Your previous response:\n{}\n\n\
             Evaluation feedback (score: {:.1}/10.0):\n{}{}\n\n\
             Reflection analysis:\n{}\n\n\
             This is improvement iteration #{}. Based on the above evaluation feedback and reflection analysis, provide a more accurate and complete response.",
            task,
            current_answer,
            critique.score,
            critique.feedback,
            suggestions_text,
            reflection,
            iteration + 1,
        )
    }
}

/// Reflection prompt builder (generates reflection text)
pub trait ReflectionPromptBuilder: Send + Sync {
    /// Build reflection prompt
    fn build_reflection_prompt(&self, task: &str, answer: &str, critique: &Critique) -> String;
}

/// Default reflection prompt builder
pub struct DefaultReflectionPromptBuilder;

impl ReflectionPromptBuilder for DefaultReflectionPromptBuilder {
    fn build_reflection_prompt(&self, task: &str, answer: &str, critique: &Critique) -> String {
        let errors_text = if critique.suggestions.is_empty() {
            String::new()
        } else {
            format!(
                "\nSpecific issues:\n{}",
                critique
                    .suggestions
                    .iter()
                    .map(|s| format!("- {}", s))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        format!(
            "Task: {}\n\n\
             Generated response:\n{}\n\n\
             Evaluation result: score {:.1}/10.0, did not pass.\n\
             Evaluation feedback: {}{}\n\n\
             Please deeply analyze the issues in the above response, considering:\n\
             1. Why did these errors or deficiencies occur?\n\
             2. What is the root cause?\n\
             3. How can similar issues be avoided next time?\n\n\
             Please output concise reflection text.",
            task, answer, critique.score, critique.feedback, errors_text,
        )
    }
}

/// Return JSON Schema for Critique structure (for LLM structured output)
pub fn critique_output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "score": {
                "type": "number",
                "description": "Quality score (0.0 to 10.0, 10.0 is highest)"
            },
            "passed": {
                "type": "boolean",
                "description": "Whether quality standards were met"
            },
            "feedback": {
                "type": "string",
                "description": "Detailed evaluation feedback, explaining strengths and weaknesses"
            },
            "suggestions": {
                "type": "array",
                "items": { "type": "string" },
                "description": "List of specific improvement suggestions"
            }
        },
        "required": ["score", "passed", "feedback"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_critique_output_parse() {
        let json = r#"{"score": 8.5, "passed": true, "feedback": "Accurate response", "suggestions": ["Could be more detailed"]}"#;
        let output: CritiqueOutput = serde_json::from_str(json).unwrap();
        assert_eq!(output.score, 8.5);
        assert!(output.passed);
        assert_eq!(output.suggestions.len(), 1);
    }

    #[test]
    fn test_critique_output_default_suggestions() {
        let json = r#"{"score": 6.0, "passed": false, "feedback": "Incomplete"}"#;
        let output: CritiqueOutput = serde_json::from_str(json).unwrap();
        assert!(output.suggestions.is_empty());
    }

    #[test]
    fn test_refinement_prompt_builder() {
        let builder = DefaultRefinementPromptBuilder;
        let critique = Critique {
            score: 5.0,
            passed: false,
            feedback: "Not accurate enough".to_string(),
            suggestions: vec!["Add examples".to_string()],
        };
        let prompt = builder.build_prompt(
            "Explain Rust",
            "Rust is...",
            &critique,
            "Needs more detail",
            0,
        );
        assert!(prompt.contains("Explain Rust"));
        assert!(prompt.contains("Not accurate enough"));
        assert!(prompt.contains("Add examples"));
        assert!(prompt.contains("improvement iteration #1"));
    }

    #[test]
    fn test_reflection_prompt_builder() {
        let builder = DefaultReflectionPromptBuilder;
        let critique = Critique {
            score: 4.0,
            passed: false,
            feedback: "Concept is incorrect".to_string(),
            suggestions: vec!["Fix the definition".to_string()],
        };
        let prompt =
            builder.build_reflection_prompt("Explain ownership", "Rust has GC...", &critique);
        assert!(prompt.contains("Explain ownership"));
        assert!(prompt.contains("Concept is incorrect"));
        assert!(prompt.contains("Fix the definition"));
    }

    #[test]
    fn test_reflection_experience() {
        let exp = ReflectionExperience::new("Confirm data before querying", "Assumes data exists")
            .with_category("database");
        assert!(exp.id.starts_with("exp_"));
        assert_eq!(exp.lesson, "Confirm data before querying");
        assert_eq!(exp.task_category, Some("database".to_string()));
    }

    #[test]
    fn test_critique_schema() {
        let schema = critique_output_schema();
        assert!(schema.is_object());
        assert!(schema["properties"]["score"].is_object());
        assert!(schema["properties"]["passed"].is_object());
    }
}
