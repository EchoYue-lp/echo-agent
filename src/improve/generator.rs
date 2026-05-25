//! Prompt improvement generator — uses LLM to regenerate prompts from failure feedback.
//!
//! Inspired by `skill-creator/scripts/improve_description.py`. Takes failed
//! eval results and generates improved system prompts or tool instructions.

use crate::agent::Agent;
use crate::improve::RunCritique;

/// Generator that uses an LLM to produce improved prompts.
pub struct PromptGenerator {
    /// Maximum characters for the generated prompt.
    pub max_chars: usize,
}

impl Default for PromptGenerator {
    fn default() -> Self {
        Self { max_chars: 4096 }
    }
}

impl PromptGenerator {
    pub fn new() -> Self { Self::default() }

    /// Generate an improved system prompt based on failure analysis.
    ///
    /// Takes the current system prompt, a set of critiques from failed runs,
    /// and asks an LLM to produce an improved version.
    pub async fn generate_improved_prompt(
        &self,
        agent: &dyn Agent,
        current_prompt: &str,
        critiques: &[RunCritique],
        task_domain: &str,
    ) -> String {
        // Collect failure patterns
        let failure_summary: String = critiques
            .iter()
            .flat_map(|c| &c.issues)
            .map(|issue| format!("- {:?}", issue))
            .collect::<Vec<_>>()
            .join("\n");

        let previous_suggestions: String = critiques
            .iter()
            .flat_map(|c| &c.suggestions)
            .map(|s| format!("- {:?}", s))
            .collect::<Vec<_>>()
            .join("\n");

        let prompt = format!(
            "You are improving a system prompt for an AI coding agent.\n\n\
             CURRENT PROMPT:\n{current_prompt}\n\n\
             DOMAIN: {task_domain}\n\n\
             FAILURE PATTERNS (issues found in previous runs):\n{failure_summary}\n\n\
             PREVIOUS SUGGESTIONS:\n{previous_suggestions}\n\n\
             TASK: Write an improved system prompt that addresses these failures.\n\
             Rules:\n\
             - Keep it under {max_chars} characters\n\
             - Be specific about tools and behaviors\n\
             - Include guardrails that prevent the observed failures\n\
             - Output ONLY the new prompt text, no explanations\n\
             - Wrap in <new_prompt>...</new_prompt> tags",
            max_chars = self.max_chars,
            current_prompt = current_prompt,
            task_domain = task_domain,
            failure_summary = failure_summary,
            previous_suggestions = previous_suggestions,
        );

        let response = agent.execute(&prompt).await;
        let raw = response.unwrap_or_else(|e| format!("Generation failed: {e}"));

        Self::extract_tagged_content(&raw, "new_prompt")
            .unwrap_or(raw)
            .chars()
            .take(self.max_chars)
            .collect()
    }

    /// Extract content between XML-style tags.
    fn extract_tagged_content(raw: &str, tag: &str) -> Option<String> {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let start = raw.find(&open)? + open.len();
        let end = raw[start..].find(&close)?;
        Some(raw[start..start + end].trim().to_string())
    }
}
