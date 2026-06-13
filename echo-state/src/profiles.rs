//! Agent and User profiles — dynamic capability and preference models.
//!
//! Provides `AgentProfile` (tracks skill proficiency and tool usage statistics)
//! and `UserProfile` (tracks user preferences, expertise areas, and common tasks).
//! Both are persisted via the framework `Store` trait.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::warn;

use echo_core::memory::Store;

use crate::skill_telemetry::SkillTelemetry;

/// Agent capability profile — tracks skill proficiency and tool usage over time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    /// Per-skill capability scores.
    pub capabilities: HashMap<String, CapabilityScore>,
    /// Per-tool usage statistics.
    pub tool_usage: HashMap<String, ToolUsageStats>,
    /// Last update timestamp (epoch milliseconds).
    pub last_updated: u64,
}

impl AgentProfile {
    /// Create a new empty profile.
    pub fn new() -> Self {
        Self {
            capabilities: HashMap::new(),
            tool_usage: HashMap::new(),
            last_updated: epoch_millis(),
        }
    }

    /// Update capabilities from a batch of skill telemetry data.
    pub fn update_from_telemetry(&mut self, telemetry: &[SkillTelemetry]) {
        for t in telemetry {
            let score = self
                .capabilities
                .entry(t.skill_name.clone())
                .or_insert_with(|| CapabilityScore {
                    skill_name: t.skill_name.clone(),
                    proficiency: 0.0,
                    usage_count: 0,
                    success_rate: 0.0,
                });

            score.usage_count = t.activation_count;
            score.success_rate = t.success_rate();

            // proficiency = success_rate × min(1.0, log10(usage_count) / 2.0)
            let usage_factor = if t.activation_count > 1 {
                (t.activation_count as f64).log10() / 2.0
            } else {
                0.0
            };
            score.proficiency = score.success_rate * usage_factor.min(1.0);
        }

        // Update tool usage from telemetry common_tools
        for t in telemetry {
            for (tool_name, count) in &t.common_tools {
                let stats =
                    self.tool_usage
                        .entry(tool_name.clone())
                        .or_insert_with(|| ToolUsageStats {
                            usage_count: 0,
                            success_count: 0,
                            common_skills: Vec::new(),
                        });
                stats.usage_count += count;
                // Estimate success_count from skill success rate
                stats.success_count += (*count as f64 * t.success_rate()) as u64;
                if !stats.common_skills.contains(&t.skill_name) {
                    stats.common_skills.push(t.skill_name.clone());
                }
            }
        }

        self.last_updated = epoch_millis();
    }

    /// Get top N capabilities sorted by proficiency (descending).
    pub fn top_capabilities(&self, n: usize) -> Vec<&CapabilityScore> {
        let mut caps: Vec<_> = self.capabilities.values().collect();
        caps.sort_by(|a, b| {
            b.proficiency
                .partial_cmp(&a.proficiency)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        caps.truncate(n);
        caps
    }

    /// Get top N tools sorted by usage count (descending).
    pub fn top_tools(&self, n: usize) -> Vec<&ToolUsageStats> {
        let mut tools: Vec<_> = self.tool_usage.values().collect();
        tools.sort_by(|a, b| b.usage_count.cmp(&a.usage_count));
        tools.truncate(n);
        tools
    }

    /// Format as a system prompt block for LLM injection.
    pub fn to_prompt_block(&self) -> String {
        if self.capabilities.is_empty() && self.tool_usage.is_empty() {
            return String::new();
        }

        let mut block = String::from("<agent-profile>\n");

        let caps = self.top_capabilities(5);
        if !caps.is_empty() {
            block.push_str("## Capabilities\n");
            for cap in &caps {
                block.push_str(&format!(
                    "- {}: {:.0}% proficiency ({} uses, {:.0}% success)\n",
                    cap.skill_name,
                    cap.proficiency * 100.0,
                    cap.usage_count,
                    cap.success_rate * 100.0,
                ));
            }
        }

        let tools = self.top_tools(5);
        if !tools.is_empty() {
            block.push_str("\n## Tool Expertise\n");
            for tool in &tools {
                let tool_name = self
                    .tool_usage
                    .iter()
                    .find(|(_, v)| std::ptr::eq(*v, *tool))
                    .map(|(k, _)| k.as_str())
                    .unwrap_or("unknown");
                block.push_str(&format!(
                    "- {}: {} uses (contexts: {})\n",
                    tool_name,
                    tool.usage_count,
                    if tool.common_skills.is_empty() {
                        "general".to_string()
                    } else {
                        tool.common_skills.join(", ")
                    },
                ));
            }
        }

        block.push_str("</agent-profile>");
        block
    }
}

impl Default for AgentProfile {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-skill capability score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityScore {
    /// Skill name.
    pub skill_name: String,
    /// Proficiency score (0.0 to 1.0), computed as success_rate × log(usage).
    pub proficiency: f64,
    /// Total activation count.
    pub usage_count: u64,
    /// Success rate (0.0 to 1.0).
    pub success_rate: f64,
}

/// Per-tool usage statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUsageStats {
    /// Total usage count across all skills.
    pub usage_count: u64,
    /// Estimated successful usage count.
    pub success_count: u64,
    /// Skills that commonly use this tool.
    pub common_skills: Vec<String>,
}

/// User preference profile — tracks user preferences, expertise, and common tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    /// User-set preferences (key-value pairs).
    pub preferences: HashMap<String, String>,
    /// Areas of expertise.
    pub expertise_areas: Vec<String>,
    /// Common task patterns with frequency.
    pub common_tasks: Vec<TaskPattern>,
    /// Last update timestamp (epoch milliseconds).
    pub last_updated: u64,
}

impl UserProfile {
    /// Create a new empty profile.
    pub fn new() -> Self {
        Self {
            preferences: HashMap::new(),
            expertise_areas: Vec::new(),
            common_tasks: Vec::new(),
            last_updated: epoch_millis(),
        }
    }

    /// Record a task type occurrence.
    pub fn record_task(&mut self, task_type: &str) {
        if let Some(pattern) = self
            .common_tasks
            .iter_mut()
            .find(|p| p.task_type == task_type)
        {
            pattern.frequency += 1;
            pattern.last_performed = epoch_millis();
        } else {
            self.common_tasks.push(TaskPattern {
                task_type: task_type.to_string(),
                frequency: 1,
                last_performed: epoch_millis(),
            });
        }
        self.last_updated = epoch_millis();
    }

    /// Set a user preference.
    pub fn set_preference(&mut self, key: &str, value: &str) {
        self.preferences.insert(key.to_string(), value.to_string());
        self.last_updated = epoch_millis();
    }

    /// Get top N common tasks sorted by frequency (descending).
    pub fn top_tasks(&self, n: usize) -> Vec<&TaskPattern> {
        let mut tasks: Vec<_> = self.common_tasks.iter().collect();
        tasks.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        tasks.truncate(n);
        tasks
    }

    /// Format as a system prompt block for LLM injection.
    pub fn to_prompt_block(&self) -> String {
        if self.preferences.is_empty()
            && self.expertise_areas.is_empty()
            && self.common_tasks.is_empty()
        {
            return String::new();
        }

        let mut block = String::from("<user-profile>\n");

        if !self.preferences.is_empty() {
            block.push_str("## Preferences\n");
            for (key, value) in &self.preferences {
                block.push_str(&format!("- {}: {}\n", key, value));
            }
        }

        if !self.expertise_areas.is_empty() {
            block.push_str("\n## Expertise\n");
            block.push_str(&self.expertise_areas.join(", "));
            block.push('\n');
        }

        let tasks = self.top_tasks(5);
        if !tasks.is_empty() {
            block.push_str("\n## Common Tasks\n");
            for task in &tasks {
                block.push_str(&format!("- {} ({}x)\n", task.task_type, task.frequency));
            }
        }

        block.push_str("</user-profile>");
        block
    }
}

impl Default for UserProfile {
    fn default() -> Self {
        Self::new()
    }
}

/// A recurring task pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPattern {
    /// Task type identifier (e.g., "coding", "data-analysis").
    pub task_type: String,
    /// How many times this task type has been performed.
    pub frequency: u64,
    /// Last performed timestamp (epoch milliseconds).
    pub last_performed: u64,
}

/// Store for profile data, backed by the framework `Store` trait.
pub struct ProfileStore {
    store: Arc<dyn Store>,
}

impl ProfileStore {
    /// Create a new profile store backed by the given `Store`.
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Load the agent profile.
    pub async fn load_agent_profile(&self) -> echo_core::error::Result<Option<AgentProfile>> {
        match self.store.get(&["agent", "profile"], "current").await {
            Ok(Some(item)) => {
                let profile: AgentProfile = serde_json::from_value(item.value).map_err(|e| {
                    echo_core::error::ReactError::Other(format!(
                        "Failed to deserialize agent profile: {}",
                        e
                    ))
                })?;
                Ok(Some(profile))
            }
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// Save the agent profile.
    pub async fn save_agent_profile(&self, profile: &AgentProfile) -> echo_core::error::Result<()> {
        let value = serde_json::to_value(profile).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to serialize agent profile: {}", e))
        })?;
        self.store
            .put(&["agent", "profile"], "current", value)
            .await
            .map_err(|e| {
                echo_core::error::ReactError::Other(format!("Failed to save agent profile: {}", e))
            })
    }

    /// Load the user profile.
    pub async fn load_user_profile(&self) -> echo_core::error::Result<Option<UserProfile>> {
        match self.store.get(&["user", "profile"], "current").await {
            Ok(Some(item)) => {
                let profile: UserProfile = serde_json::from_value(item.value).map_err(|e| {
                    echo_core::error::ReactError::Other(format!(
                        "Failed to deserialize user profile: {}",
                        e
                    ))
                })?;
                Ok(Some(profile))
            }
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// Save the user profile.
    pub async fn save_user_profile(&self, profile: &UserProfile) -> echo_core::error::Result<()> {
        let value = serde_json::to_value(profile).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to serialize user profile: {}", e))
        })?;
        self.store
            .put(&["user", "profile"], "current", value)
            .await
            .map_err(|e| {
                echo_core::error::ReactError::Other(format!("Failed to save user profile: {}", e))
            })
    }
}

/// Get current time as epoch milliseconds.
fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_telemetry::SkillTelemetry;

    fn make_telemetry(name: &str, count: u64, success: u64) -> SkillTelemetry {
        SkillTelemetry {
            skill_name: name.to_string(),
            activation_count: count,
            success_count: success,
            failure_count: count - success,
            total_duration_ms: count * 3000,
            common_tools: {
                let mut m = HashMap::new();
                m.insert("bash".to_string(), count);
                m.insert("read_file".to_string(), count / 2);
                m
            },
            common_failures: Vec::new(),
            last_used: epoch_millis(),
            first_used: epoch_millis() - 86400000,
        }
    }

    #[test]
    fn test_agent_profile_new() {
        let p = AgentProfile::new();
        assert!(p.capabilities.is_empty());
        assert!(p.tool_usage.is_empty());
    }

    #[test]
    fn test_agent_profile_update_from_telemetry() {
        let mut p = AgentProfile::new();
        let telemetry = vec![
            make_telemetry("coding", 100, 92),
            make_telemetry("paper-search", 10, 8),
        ];
        p.update_from_telemetry(&telemetry);

        assert_eq!(p.capabilities.len(), 2);
        assert_eq!(p.tool_usage.len(), 2);

        let coding = p.capabilities.get("coding").unwrap();
        assert_eq!(coding.usage_count, 100);
        assert!((coding.success_rate - 0.92).abs() < f64::EPSILON);
        assert!(coding.proficiency > 0.0);

        // bash used by both skills
        let bash = p.tool_usage.get("bash").unwrap();
        assert_eq!(bash.usage_count, 110); // 100 + 10
    }

    #[test]
    fn test_agent_profile_top_capabilities() {
        let mut p = AgentProfile::new();
        let telemetry = vec![
            make_telemetry("coding", 100, 95),
            make_telemetry("paper-search", 5, 4),
            make_telemetry("data-viz", 50, 45),
        ];
        p.update_from_telemetry(&telemetry);

        let top = p.top_capabilities(2);
        assert_eq!(top.len(), 2);
        // coding should be first (highest proficiency due to high usage + success)
        assert_eq!(top[0].skill_name, "coding");
    }

    #[test]
    fn test_agent_profile_prompt_block() {
        let mut p = AgentProfile::new();
        let telemetry = vec![make_telemetry("coding", 100, 95)];
        p.update_from_telemetry(&telemetry);

        let block = p.to_prompt_block();
        assert!(block.contains("<agent-profile>"));
        assert!(block.contains("coding"));
        assert!(block.contains("Capabilities"));
        assert!(block.contains("</agent-profile>"));
    }

    #[test]
    fn test_agent_profile_empty_prompt() {
        let p = AgentProfile::new();
        assert!(p.to_prompt_block().is_empty());
    }

    #[test]
    fn test_user_profile_new() {
        let p = UserProfile::new();
        assert!(p.preferences.is_empty());
        assert!(p.expertise_areas.is_empty());
        assert!(p.common_tasks.is_empty());
    }

    #[test]
    fn test_user_profile_set_preference() {
        let mut p = UserProfile::new();
        p.set_preference("language", "zh-CN");
        p.set_preference("code_style", "concise");
        assert_eq!(p.preferences.len(), 2);
        assert_eq!(p.preferences.get("language").unwrap(), "zh-CN");
    }

    #[test]
    fn test_user_profile_record_task() {
        let mut p = UserProfile::new();
        p.record_task("coding");
        p.record_task("coding");
        p.record_task("data-analysis");
        assert_eq!(p.common_tasks.len(), 2);
        assert_eq!(p.common_tasks[0].frequency, 2);
    }

    #[test]
    fn test_user_profile_top_tasks() {
        let mut p = UserProfile::new();
        p.record_task("coding");
        p.record_task("coding");
        p.record_task("coding");
        p.record_task("data-analysis");
        let top = p.top_tasks(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].task_type, "coding");
        assert_eq!(top[0].frequency, 3);
    }

    #[test]
    fn test_user_profile_prompt_block() {
        let mut p = UserProfile::new();
        p.set_preference("language", "zh-CN");
        p.expertise_areas.push("Rust".to_string());
        p.record_task("coding");
        let block = p.to_prompt_block();
        assert!(block.contains("<user-profile>"));
        assert!(block.contains("language: zh-CN"));
        assert!(block.contains("Rust"));
        assert!(block.contains("</user-profile>"));
    }

    #[test]
    fn test_user_profile_empty_prompt() {
        let p = UserProfile::new();
        assert!(p.to_prompt_block().is_empty());
    }
}
