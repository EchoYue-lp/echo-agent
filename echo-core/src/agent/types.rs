//! Shared agent type definitions

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
    fn test_critique_schema() {
        let schema = critique_output_schema();
        assert!(schema.is_object());
        assert!(schema["properties"]["score"].is_object());
        assert!(schema["properties"]["passed"].is_object());
    }
}
