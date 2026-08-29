//! Serde-based configuration and wire-format conversion.

use crate::errors::{LearningError, required_text};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub max_iterations: usize,
    #[serde(default)]
    pub tools: Vec<String>,
}

impl AgentProfile {
    /// Structural deserialization and domain validation are separate steps.
    pub fn validate(&self) -> Result<(), LearningError> {
        required_text("name", Some(&self.name))?;
        if self.max_iterations == 0 {
            return Err(LearningError::InvalidLimit(self.max_iterations.to_string()));
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("agent profile is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("agent profile violates a domain rule: {0}")]
    Validation(#[from] LearningError),
}

pub fn profile_to_json(profile: &AgentProfile) -> serde_json::Result<String> {
    serde_json::to_string_pretty(profile)
}

pub fn profile_from_json(input: &str) -> serde_json::Result<AgentProfile> {
    serde_json::from_str(input)
}

pub fn parse_and_validate_profile(input: &str) -> Result<AgentProfile, ProfileError> {
    let profile = profile_from_json(input)?;
    profile.validate()?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_round_trips() -> serde_json::Result<()> {
        let profile = AgentProfile {
            name: "assistant".to_string(),
            max_iterations: 8,
            tools: vec!["search".to_string()],
        };
        let json = profile_to_json(&profile)?;
        assert_eq!(profile_from_json(&json)?, profile);
        Ok(())
    }

    #[test]
    fn validation_is_distinct_from_json_parsing() -> Result<(), ProfileError> {
        let invalid = r#"{"name":" ","max_iterations":0}"#;
        assert!(matches!(
            parse_and_validate_profile(invalid),
            Err(ProfileError::Validation(LearningError::MissingField(
                "name"
            )))
        ));
        assert!(matches!(
            parse_and_validate_profile(r#"{"name":"assistant","max_iterations":0}"#),
            Err(ProfileError::Validation(LearningError::InvalidLimit(_)))
        ));

        assert!(matches!(
            parse_and_validate_profile("not json"),
            Err(ProfileError::Json(_))
        ));
        Ok(())
    }
}
