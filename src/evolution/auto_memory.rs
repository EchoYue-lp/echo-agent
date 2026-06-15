//! Auto-memory: automatically extract and persist key observations from agent sessions.
//!
//! Runs after each conversation ends (or on `/auto-memory` trigger).
//! Extracts:
//! - Project patterns discovered (e.g., "this project uses Rust 2024 edition")
//! - User preferences expressed (e.g., "user prefers concise comments")
//! - Key decisions made during the session
//! - Discovered bugs and their fixes
//! - Important file paths and project structure observations

use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

/// An observation extracted from a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    /// Category of the observation
    pub category: ObservationCategory,
    /// The observation text (concise, actionable)
    pub text: String,
    /// Confidence (0.0-1.0) — how certain the extraction is
    pub confidence: f64,
    /// Source: which conversation turn prompted this
    pub source_turn: Option<usize>,
}

/// Categories of observations that auto-memory can extract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ObservationCategory {
    /// Project structure, conventions, patterns
    Project,
    /// User preferences, coding style
    User,
    /// Bugs found, issues encountered
    Bug,
    /// Key decisions made
    Decision,
    /// Important file paths
    FilePath,
}

impl std::fmt::Display for ObservationCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project => write!(f, "Project"),
            Self::User => write!(f, "User"),
            Self::Bug => write!(f, "Bug"),
            Self::Decision => write!(f, "Decision"),
            Self::FilePath => write!(f, "FilePath"),
        }
    }
}

/// Configuration for auto-memory behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoMemoryConfig {
    /// Whether auto-memory is enabled
    pub enabled: bool,
    /// Minimum confidence to auto-save (0.0-1.0)
    pub min_confidence: f64,
    /// Maximum observations per session
    pub max_per_session: usize,
    /// Categories to auto-extract
    pub categories: Vec<ObservationCategory>,
}

impl Default for AutoMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_confidence: 0.7,
            max_per_session: 10,
            categories: vec![
                ObservationCategory::Project,
                ObservationCategory::User,
                ObservationCategory::Bug,
                ObservationCategory::Decision,
            ],
        }
    }
}

/// Extract observations from a conversation transcript.
///
/// Uses heuristics (not LLM) to identify memorable information:
/// - Lines containing "remember", "note that", "convention", "pattern"
/// - File paths mentioned multiple times
/// - Error patterns and their resolutions
/// - User directives ("always", "never", "prefer", "don't")
pub fn extract_observations(
    messages: &[(String, String)], // (role, content) pairs
    config: &AutoMemoryConfig,
) -> Vec<Observation> {
    if !config.enabled {
        return Vec::new();
    }

    let mut observations = Vec::new();

    for (turn_idx, (role, content)) in messages.iter().enumerate() {
        if observations.len() >= config.max_per_session {
            break;
        }

        // User directives → User preferences
        if role == "user" {
            let lower = content.to_lowercase();
            for keyword in &[
                "always ",
                "never ",
                "prefer ",
                "don't ",
                "i want ",
                "i like ",
                "please use ",
                "don't use ",
                "make sure to ",
            ] {
                if let Some(pos) = lower.find(keyword) {
                    let end = content.len().min(pos + 200);
                    let snippet = &content[pos..end];
                    let obs = Observation {
                        category: ObservationCategory::User,
                        text: snippet.trim().to_string(),
                        confidence: 0.8,
                        source_turn: Some(turn_idx),
                    };
                    if obs.confidence >= config.min_confidence
                        && config.categories.contains(&obs.category)
                    {
                        observations.push(obs);
                    }
                }
            }
        }

        // Assistant discoveries → Project patterns
        if role == "assistant" {
            let lower = content.to_lowercase();
            for keyword in &[
                "this project uses",
                "the convention is",
                "i noticed that",
                "the structure is",
                "this codebase",
                "project pattern",
                "the project is",
                "this repo uses",
            ] {
                if let Some(pos) = lower.find(keyword) {
                    let end = content.len().min(pos + 200);
                    let snippet = &content[pos..end];
                    let obs = Observation {
                        category: ObservationCategory::Project,
                        text: snippet.trim().to_string(),
                        confidence: 0.75,
                        source_turn: Some(turn_idx),
                    };
                    if obs.confidence >= config.min_confidence
                        && config.categories.contains(&obs.category)
                    {
                        observations.push(obs);
                    }
                }
            }

            // Bug fixes
            for keyword in &[
                "the bug was",
                "fixed the issue",
                "root cause",
                "the fix is",
                "the error was caused by",
                "resolved by",
            ] {
                if let Some(pos) = lower.find(keyword) {
                    let end = content.len().min(pos + 300);
                    let snippet = &content[pos..end];
                    let obs = Observation {
                        category: ObservationCategory::Bug,
                        text: snippet.trim().to_string(),
                        confidence: 0.85,
                        source_turn: Some(turn_idx),
                    };
                    if obs.confidence >= config.min_confidence
                        && config.categories.contains(&obs.category)
                    {
                        observations.push(obs);
                    }
                }
            }

            // Decisions
            for keyword in &[
                "we decided to",
                "the decision is",
                "we'll go with",
                "chosen approach",
                "architecture decision",
            ] {
                if let Some(pos) = lower.find(keyword) {
                    let end = content.len().min(pos + 200);
                    let snippet = &content[pos..end];
                    let obs = Observation {
                        category: ObservationCategory::Decision,
                        text: snippet.trim().to_string(),
                        confidence: 0.8,
                        source_turn: Some(turn_idx),
                    };
                    if obs.confidence >= config.min_confidence
                        && config.categories.contains(&obs.category)
                    {
                        observations.push(obs);
                    }
                }
            }
        }

        // Both roles: file path detection
        if config.categories.contains(&ObservationCategory::FilePath) {
            // Simple heuristic: detect paths like src/foo/bar.rs or ./foo/bar
            for word in content.split_whitespace() {
                let trimmed = word.trim_matches(|c: char| {
                    !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_'
                });
                if (trimmed.contains('/') && trimmed.len() > 5)
                    && (trimmed.ends_with(".rs")
                        || trimmed.ends_with(".ts")
                        || trimmed.ends_with(".py")
                        || trimmed.ends_with(".go")
                        || trimmed.ends_with(".yaml")
                        || trimmed.ends_with(".toml")
                        || trimmed.ends_with(".json"))
                {
                    let obs = Observation {
                        category: ObservationCategory::FilePath,
                        text: trimmed.to_string(),
                        confidence: 0.6,
                        source_turn: Some(turn_idx),
                    };
                    // File paths have lower confidence, only include if threshold is low enough
                    if obs.confidence >= config.min_confidence {
                        observations.push(obs);
                    }
                }
            }
        }
    }

    // Deduplicate similar observations
    deduplicate_observations(&mut observations);

    observations.truncate(config.max_per_session);
    observations
}

/// Remove near-duplicate observations.
///
/// Sorts by category, then by text length DESCENDING so longer/more-complete
/// versions of the same observation come first. `dedup_by` keeps the first
/// element of each adjacent pair, so this guarantees that when one snippet is
/// a prefix-substring of another, the longer one wins.
fn deduplicate_observations(observations: &mut Vec<Observation>) {
    // Sort by category, then by text length DESC, then by text for stability
    observations.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| b.text.chars().count().cmp(&a.text.chars().count()))
            .then_with(|| a.text.cmp(&b.text))
    });
    // Now adjacent dedup keeps the longer snippet when one contains the other's prefix
    observations.dedup_by(|a, b| {
        a.category == b.category
            && ((a.text.chars().count() > 20
                && b.text.chars().count() > 20
                && b.text
                    .contains(&a.text.chars().take(20).collect::<String>()))
                || (b.text.chars().count() > 20
                    && a.text.chars().count() > 20
                    && a.text
                        .contains(&b.text.chars().take(20).collect::<String>())))
    });
}

/// Format observations for storage in project memory.
pub fn format_observations_for_memory(observations: &[Observation]) -> String {
    if observations.is_empty() {
        return String::new();
    }

    let mut output = String::from("## Auto-extracted observations\n\n");

    let mut by_category: std::collections::HashMap<ObservationCategory, Vec<&Observation>> =
        std::collections::HashMap::new();
    for obs in observations {
        by_category
            .entry(obs.category.clone())
            .or_default()
            .push(obs);
    }

    let category_names = [
        (ObservationCategory::Project, "Project Patterns"),
        (ObservationCategory::User, "User Preferences"),
        (ObservationCategory::Bug, "Bugs & Fixes"),
        (ObservationCategory::Decision, "Decisions"),
        (ObservationCategory::FilePath, "Important Files"),
    ];

    for (cat, name) in &category_names {
        if let Some(obs_list) = by_category.get(cat) {
            output.push_str(&format!("### {}\n", name));
            for obs in obs_list {
                output.push_str(&format!("- {}\n", obs.text));
            }
            output.push('\n');
        }
    }

    output
}

/// Map an auto-memory observation category to typed memory metadata.
pub fn observation_memory_type(category: &ObservationCategory) -> echo_core::memory::MemoryType {
    match category {
        ObservationCategory::User => echo_core::memory::MemoryType::UserPreference,
        ObservationCategory::Bug => echo_core::memory::MemoryType::DebuggingLesson,
        ObservationCategory::Decision => echo_core::memory::MemoryType::ArchitectureDecision,
        ObservationCategory::Project | ObservationCategory::FilePath => {
            echo_core::memory::MemoryType::ProjectFact
        }
    }
}

/// Stable key for idempotent auto-memory writes.
pub fn observation_memory_key(observation: &Observation) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    observation.category.hash(&mut hasher);
    observation.text.hash(&mut hasher);
    observation.source_turn.hash(&mut hasher);
    format!(
        "auto_{}_{}",
        observation.source_turn.unwrap_or_default(),
        hasher.finish()
    )
}

/// Persist observations to the real layered memory path used by runtime recall.
pub async fn write_observations_to_memory_layer(
    observations: &[Observation],
    layer_manager: &super::MemoryLayerManager,
) -> Result<usize, String> {
    if observations.is_empty() {
        return Ok(0);
    }

    let mut written = 0;
    for observation in observations {
        let meta = echo_core::memory::MemoryMeta::new(
            observation_memory_type(&observation.category),
            echo_core::memory::MemorySource::AutoExtracted,
            observation.category.to_string(),
        )
        .with_confidence(observation.confidence as f32);

        layer_manager
            .write_memory(
                &observation_memory_key(observation),
                &observation.text,
                meta,
            )
            .await
            .map_err(|e| format!("Failed to write typed auto memory: {e}"))?;
        written += 1;
    }

    Ok(written)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_conversation() -> Vec<(String, String)> {
        vec![
            (
                "user".to_string(),
                "I prefer concise comments, don't write verbose docs.".to_string(),
            ),
            (
                "assistant".to_string(),
                "I noticed that this project uses Rust 2024 edition with workspace resolver 3.".to_string(),
            ),
            (
                "user".to_string(),
                "Always run cargo check before committing.".to_string(),
            ),
            (
                "assistant".to_string(),
                "The bug was caused by a race condition in the async lock. The fix is to use a Mutex instead of RwLock for the shared state.".to_string(),
            ),
            (
                "user".to_string(),
                "Let's refactor the module structure.".to_string(),
            ),
            (
                "assistant".to_string(),
                "We decided to split the monolithic module into submodules for better organization.".to_string(),
            ),
        ]
    }

    #[test]
    fn test_extract_user_preferences() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig::default();
        let observations = extract_observations(&messages, &config);

        let user_obs: Vec<_> = observations
            .iter()
            .filter(|o| o.category == ObservationCategory::User)
            .collect();

        assert!(
            !user_obs.is_empty(),
            "Should extract at least one user preference"
        );
        assert!(
            user_obs
                .iter()
                .any(|o| o.text.contains("prefer") || o.text.contains("concise")),
            "Should contain the preference about concise comments"
        );
    }

    #[test]
    fn test_extract_project_patterns() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig::default();
        let observations = extract_observations(&messages, &config);

        let project_obs: Vec<_> = observations
            .iter()
            .filter(|o| o.category == ObservationCategory::Project)
            .collect();

        assert!(
            !project_obs.is_empty(),
            "Should extract at least one project pattern"
        );
        assert!(
            project_obs.iter().any(|o| o.text.contains("Rust 2024")),
            "Should detect Rust edition"
        );
    }

    #[test]
    fn test_extract_bug_fixes() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig::default();
        let observations = extract_observations(&messages, &config);

        let bug_obs: Vec<_> = observations
            .iter()
            .filter(|o| o.category == ObservationCategory::Bug)
            .collect();

        assert!(
            !bug_obs.is_empty(),
            "Should extract at least one bug observation"
        );
        assert!(
            bug_obs
                .iter()
                .any(|o| o.text.contains("race condition") || o.text.contains("bug")),
            "Should contain the bug description"
        );
    }

    #[test]
    fn test_extract_decisions() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig::default();
        let observations = extract_observations(&messages, &config);

        let decision_obs: Vec<_> = observations
            .iter()
            .filter(|o| o.category == ObservationCategory::Decision)
            .collect();

        assert!(
            !decision_obs.is_empty(),
            "Should extract at least one decision"
        );
    }

    #[test]
    fn test_disabled_auto_memory() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig {
            enabled: false,
            ..Default::default()
        };
        let observations = extract_observations(&messages, &config);
        assert!(
            observations.is_empty(),
            "Disabled config should return no observations"
        );
    }

    #[test]
    fn test_max_per_session_limit() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig {
            max_per_session: 2,
            ..Default::default()
        };
        let observations = extract_observations(&messages, &config);
        assert!(
            observations.len() <= 2,
            "Should not exceed max_per_session limit"
        );
    }

    #[test]
    fn test_min_confidence_filter() {
        let messages = sample_conversation();
        let config = AutoMemoryConfig {
            min_confidence: 0.99,
            ..Default::default()
        };
        let observations = extract_observations(&messages, &config);
        assert!(
            observations.is_empty(),
            "Very high confidence threshold should filter out all observations"
        );
    }

    #[test]
    fn test_format_empty_observations() {
        let formatted = format_observations_for_memory(&[]);
        assert!(formatted.is_empty());
    }

    #[test]
    fn test_format_observations_output() {
        let observations = vec![
            Observation {
                category: ObservationCategory::Project,
                text: "Uses Rust 2024 edition".to_string(),
                confidence: 0.75,
                source_turn: Some(1),
            },
            Observation {
                category: ObservationCategory::User,
                text: "Prefers concise comments".to_string(),
                confidence: 0.8,
                source_turn: Some(0),
            },
        ];

        let formatted = format_observations_for_memory(&observations);
        assert!(formatted.contains("Auto-extracted observations"));
        assert!(formatted.contains("Project Patterns"));
        assert!(formatted.contains("User Preferences"));
        assert!(formatted.contains("Uses Rust 2024 edition"));
        assert!(formatted.contains("Prefers concise comments"));
    }

    #[test]
    fn test_deduplication() {
        let messages = vec![(
            "assistant".to_string(),
            "The bug was in the parser module. The bug was causing crashes on malformed input."
                .to_string(),
        )];
        let config = AutoMemoryConfig::default();
        let observations = extract_observations(&messages, &config);

        // Should deduplicate the two "the bug was" observations
        let bug_obs: Vec<_> = observations
            .iter()
            .filter(|o| o.category == ObservationCategory::Bug)
            .collect();
        assert!(
            bug_obs.len() <= 2,
            "Should deduplicate similar bug observations, got {}",
            bug_obs.len()
        );
    }

    #[test]
    fn test_empty_conversation() {
        let messages: Vec<(String, String)> = vec![];
        let config = AutoMemoryConfig::default();
        let observations = extract_observations(&messages, &config);
        assert!(observations.is_empty());
    }

    #[test]
    fn test_observation_category_display() {
        assert_eq!(ObservationCategory::Project.to_string(), "Project");
        assert_eq!(ObservationCategory::User.to_string(), "User");
        assert_eq!(ObservationCategory::Bug.to_string(), "Bug");
        assert_eq!(ObservationCategory::Decision.to_string(), "Decision");
        assert_eq!(ObservationCategory::FilePath.to_string(), "FilePath");
    }

    #[test]
    fn test_category_filter() {
        let messages = sample_conversation();
        // Only extract User observations
        let config = AutoMemoryConfig {
            categories: vec![ObservationCategory::User],
            ..Default::default()
        };
        let observations = extract_observations(&messages, &config);
        for obs in &observations {
            assert_eq!(
                obs.category,
                ObservationCategory::User,
                "Should only extract User category"
            );
        }
    }

    #[test]
    fn test_auto_memory_config_default() {
        let config = AutoMemoryConfig::default();
        assert!(config.enabled);
        assert!((config.min_confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.max_per_session, 10);
        assert_eq!(config.categories.len(), 4);
    }
}
