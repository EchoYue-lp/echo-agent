//! Skill execution telemetry — tracks tool reliability while skills are active.
//!
//! Provides `SkillTelemetryStore` which wraps the framework `Store` trait to persist
//! telemetry data under the `["agent", "skill_telemetry"]` namespace.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::warn;

use echo_core::memory::Store;

/// A single tool execution observed while a skill is active.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionRecord {
    /// Name of the skill that was activated.
    pub skill_name: String,
    /// Session identifier.
    pub session_id: String,
    /// When the skill was activated (epoch milliseconds).
    pub activated_at: u64,
    /// How long the skill was active (milliseconds).
    pub duration_ms: u64,
    /// Tools used during this skill activation.
    pub tools_used: Vec<String>,
    /// Total number of tool calls during this activation.
    pub tool_calls_count: usize,
    /// Whether the skill execution was successful.
    pub success: bool,
    /// Error message if the execution failed.
    pub error_message: Option<String>,
}

/// Aggregated telemetry metrics for a single skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTelemetry {
    /// Skill name.
    pub skill_name: String,
    /// Total number of tool observations while this skill was active.
    pub activation_count: u64,
    /// Number of successful activations.
    pub success_count: u64,
    /// Number of failed activations.
    pub failure_count: u64,
    /// Total cumulative duration (milliseconds).
    pub total_duration_ms: u64,
    /// Tools commonly used with this skill and their usage counts.
    pub common_tools: HashMap<String, u64>,
    /// Common failure patterns.
    pub common_failures: Vec<FailurePattern>,
    /// Last used timestamp (epoch milliseconds).
    pub last_used: u64,
    /// First used timestamp (epoch milliseconds).
    pub first_used: u64,
    /// Recent execution results (last 20 executions, true = success, false = failure).
    /// Used for computing recent_success_rate in health monitoring.
    #[serde(default)]
    pub recent_records: VecDeque<bool>,
}

impl SkillTelemetry {
    /// Create new empty telemetry for a skill.
    pub fn new(skill_name: &str) -> Self {
        let now = epoch_millis();
        Self {
            skill_name: skill_name.to_string(),
            activation_count: 0,
            success_count: 0,
            failure_count: 0,
            total_duration_ms: 0,
            common_tools: HashMap::new(),
            common_failures: Vec::new(),
            last_used: now,
            first_used: now,
            recent_records: VecDeque::new(),
        }
    }

    /// Record a single execution.
    pub fn record(&mut self, record: &SkillExecutionRecord) {
        self.activation_count += 1;
        if record.success {
            self.success_count += 1;
        } else {
            self.failure_count += 1;
            // Track failure patterns
            if let Some(ref msg) = record.error_message {
                let snippet = truncate_to_200(msg);
                if let Some(pattern) = self
                    .common_failures
                    .iter_mut()
                    .find(|f| f.error_snippet == snippet)
                {
                    pattern.count += 1;
                    pattern.last_occurred = record.activated_at;
                } else {
                    self.common_failures.push(FailurePattern {
                        error_snippet: snippet,
                        count: 1,
                        last_occurred: record.activated_at,
                    });
                }
            }
        }
        self.total_duration_ms += record.duration_ms;
        for tool in &record.tools_used {
            *self.common_tools.entry(tool.clone()).or_insert(0) += 1;
        }
        self.last_used = record.activated_at;
        if record.activated_at < self.first_used {
            self.first_used = record.activated_at;
        }

        // Maintain recent_records FIFO (last 20 executions)
        self.recent_records.push_front(record.success);
        if self.recent_records.len() > 20 {
            self.recent_records.pop_back();
        }
    }

    /// Success rate as a percentage (0.0 to 1.0).
    pub fn success_rate(&self) -> f64 {
        if self.activation_count == 0 {
            return 0.0;
        }
        self.success_count as f64 / self.activation_count as f64
    }

    /// Average duration in milliseconds.
    pub fn avg_duration_ms(&self) -> u64 {
        if self.activation_count == 0 {
            return 0;
        }
        self.total_duration_ms / self.activation_count
    }

    /// Recent success rate based on the last 20 executions (0.0 to 1.0).
    /// Returns 0.0 if no recent records are available.
    pub fn recent_success_rate(&self) -> f64 {
        if self.recent_records.is_empty() {
            return 0.0;
        }
        let successes = self.recent_records.iter().filter(|&&s| s).count();
        successes as f64 / self.recent_records.len() as f64
    }
}

/// A recurring failure pattern observed during skill execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailurePattern {
    /// First 200 characters of the error message.
    pub error_snippet: String,
    /// How many times this failure has occurred.
    pub count: u64,
    /// Last occurrence timestamp (epoch milliseconds).
    pub last_occurred: u64,
}

/// Store for skill telemetry data, backed by the framework `Store` trait.
///
/// Stores aggregated telemetry under `["agent", "skill_telemetry"]` namespace,
/// keyed by skill name.
pub struct SkillTelemetryStore {
    store: Arc<dyn Store>,
}

impl SkillTelemetryStore {
    /// Create a new telemetry store backed by the given `Store`.
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self { store }
    }

    /// Record a skill execution, updating aggregated telemetry.
    pub async fn record_execution(
        &self,
        record: &SkillExecutionRecord,
    ) -> echo_core::error::Result<()> {
        let mut telemetry = self
            .get_telemetry(&record.skill_name)
            .await?
            .unwrap_or_else(|| SkillTelemetry::new(&record.skill_name));

        telemetry.record(record);

        let value = serde_json::to_value(&telemetry).map_err(|e| {
            echo_core::error::ReactError::Other(format!("Failed to serialize telemetry: {}", e))
        })?;

        self.store
            .put(&["agent", "skill_telemetry"], &record.skill_name, value)
            .await
            .map_err(|e| {
                echo_core::error::ReactError::Other(format!("Failed to store telemetry: {}", e))
            })?;

        Ok(())
    }

    /// Get aggregated telemetry for a specific skill.
    pub async fn get_telemetry(
        &self,
        skill_name: &str,
    ) -> echo_core::error::Result<Option<SkillTelemetry>> {
        match self
            .store
            .get(&["agent", "skill_telemetry"], skill_name)
            .await
        {
            Ok(Some(item)) => {
                let telemetry: SkillTelemetry =
                    serde_json::from_value(item.value).map_err(|e| {
                        echo_core::error::ReactError::Other(format!(
                            "Failed to deserialize telemetry for '{}': {}",
                            skill_name, e
                        ))
                    })?;
                Ok(Some(telemetry))
            }
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// List telemetry for all skills.
    pub async fn list_all(&self) -> echo_core::error::Result<Vec<SkillTelemetry>> {
        let items = self
            .store
            .list(&["agent", "skill_telemetry"])
            .await
            .unwrap_or_default();

        let mut result = Vec::new();
        for item in items {
            match serde_json::from_value::<SkillTelemetry>(item.value) {
                Ok(t) => result.push(t),
                Err(e) => {
                    warn!("Failed to deserialize telemetry entry: {}", e);
                }
            }
        }

        result.sort_by_key(|telemetry| std::cmp::Reverse(telemetry.last_used));
        Ok(result)
    }
}

/// Get current time as epoch milliseconds.
fn epoch_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Truncate a string to at most 200 characters.
fn truncate_to_200(s: &str) -> String {
    if s.chars().count() <= 200 {
        s.to_string()
    } else {
        s.chars().take(200).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(name: &str, success: bool, duration: u64) -> SkillExecutionRecord {
        SkillExecutionRecord {
            skill_name: name.to_string(),
            session_id: "test-session".to_string(),
            activated_at: epoch_millis(),
            duration_ms: duration,
            tools_used: vec!["bash".into(), "read_file".into()],
            tool_calls_count: 2,
            success,
            error_message: if success {
                None
            } else {
                Some("timeout error".into())
            },
        }
    }

    #[test]
    fn test_telemetry_new() {
        let t = SkillTelemetry::new("test-skill");
        assert_eq!(t.skill_name, "test-skill");
        assert_eq!(t.activation_count, 0);
        assert_eq!(t.success_rate(), 0.0);
    }

    #[test]
    fn test_telemetry_record_success() {
        let mut t = SkillTelemetry::new("coding");
        let r = make_record("coding", true, 5000);
        t.record(&r);
        assert_eq!(t.activation_count, 1);
        assert_eq!(t.success_count, 1);
        assert_eq!(t.failure_count, 0);
        assert!((t.success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_telemetry_record_failure() {
        let mut t = SkillTelemetry::new("coding");
        let r = make_record("coding", false, 3000);
        t.record(&r);
        assert_eq!(t.activation_count, 1);
        assert_eq!(t.success_count, 0);
        assert_eq!(t.failure_count, 1);
        assert_eq!(t.common_failures.len(), 1);
        assert_eq!(t.common_failures[0].error_snippet, "timeout error");
    }

    #[test]
    fn failure_snippet_truncation_is_utf8_safe() {
        let input = "错".repeat(250);
        let truncated = truncate_to_200(&input);
        assert_eq!(truncated.chars().count(), 200);
    }

    #[test]
    fn test_telemetry_aggregate() {
        let mut t = SkillTelemetry::new("coding");
        t.record(&make_record("coding", true, 5000));
        t.record(&make_record("coding", true, 3000));
        t.record(&make_record("coding", false, 1000));

        assert_eq!(t.activation_count, 3);
        assert_eq!(t.success_count, 2);
        assert_eq!(t.failure_count, 1);
        assert_eq!(t.total_duration_ms, 9000);
        assert_eq!(t.avg_duration_ms(), 3000);
        // bash used 3 times, read_file used 3 times
        assert_eq!(t.common_tools.get("bash"), Some(&3));
        assert_eq!(t.common_tools.get("read_file"), Some(&3));
    }

    #[test]
    fn test_failure_pattern_dedup() {
        let mut t = SkillTelemetry::new("coding");
        t.record(&make_record("coding", false, 1000));
        t.record(&make_record("coding", false, 2000));
        assert_eq!(t.common_failures.len(), 1);
        assert_eq!(t.common_failures[0].count, 2);
    }

    #[test]
    fn test_truncate_to_200() {
        let short = "hello";
        assert_eq!(truncate_to_200(short), "hello");

        let long = "a".repeat(300);
        assert_eq!(truncate_to_200(&long).len(), 200);
    }

    #[test]
    fn test_recent_records_fifo() {
        let mut t = SkillTelemetry::new("coding");

        // Record 25 executions (more than the 20 FIFO limit)
        for i in 0..25 {
            let r = make_record("coding", i % 2 == 0, 1000);
            t.record(&r);
        }

        // Should only keep last 20
        assert_eq!(t.recent_records.len(), 20);
        assert_eq!(t.activation_count, 25);
    }

    #[test]
    fn test_recent_success_rate_empty() {
        let t = SkillTelemetry::new("coding");
        assert_eq!(t.recent_success_rate(), 0.0);
    }

    #[test]
    fn test_recent_success_rate_all_success() {
        let mut t = SkillTelemetry::new("coding");
        for _ in 0..5 {
            t.record(&make_record("coding", true, 1000));
        }
        assert!((t.recent_success_rate() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_recent_success_rate_mixed() {
        let mut t = SkillTelemetry::new("coding");
        // 3 success, 2 failure
        t.record(&make_record("coding", true, 1000));
        t.record(&make_record("coding", true, 1000));
        t.record(&make_record("coding", false, 1000));
        t.record(&make_record("coding", true, 1000));
        t.record(&make_record("coding", false, 1000));
        assert!((t.recent_success_rate() - 0.6).abs() < f64::EPSILON);
    }
}
