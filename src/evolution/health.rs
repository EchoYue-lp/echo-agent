//! Skill health monitoring and scoring.
//!
//! Monitors the health of active skills by analyzing telemetry data including
//! success rates, usage patterns, and failure modes. Provides health scores
//! that can drive skill lifecycle decisions (e.g., deprecation).

use chrono::{DateTime, Utc};
use std::sync::Arc;

use echo_core::memory::store::Store;
use echo_state::skill_telemetry::{SkillTelemetry, SkillTelemetryStore};
use serde::{Deserialize, Serialize};

use super::audit::{ChangeEntryBuilder, ChangeLog, ChangeType, EntityType};
use crate::error::Result;

// Re-export SkillDescriptor for use in this module.
pub use echo_execution::skills::external::SkillDescriptor;

/// Health status classification for a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// Skill is healthy (score >= 0.7).
    Healthy,
    /// Skill needs attention (score 0.4-0.7).
    NeedsAttention,
    /// Skill is unhealthy (score < 0.4).
    Unhealthy,
}

impl HealthStatus {
    /// Convert a health score (0.0-1.0) to a health status.
    pub fn from_score(score: f64) -> Self {
        if score >= 0.7 {
            Self::Healthy
        } else if score >= 0.4 {
            Self::NeedsAttention
        } else {
            Self::Unhealthy
        }
    }

    /// Returns a human-readable description of the health status.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::NeedsAttention => "needs attention",
            Self::Unhealthy => "unhealthy",
        }
    }
}

/// Detailed breakdown of health score components.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthBreakdown {
    /// Overall success rate (0.0-1.0).
    pub success_rate: f64,
    /// Recent success rate from last 20 executions (0.0-1.0).
    pub recent_success_rate: f64,
    /// Usage frequency normalized to 0.0-1.0 range.
    pub usage_frequency: f64,
    /// Freshness based on days since last use (0.0-1.0, 1.0 = very recent).
    pub freshness: f64,
    /// User approval inferred from low failure rate (0.0-1.0).
    pub user_approval: f64,
    /// Command validity - absence of critical failures (0.0-1.0).
    pub command_validity: f64,
}

impl HealthBreakdown {
    /// Compute overall health score using weighted formula:
    /// health = success_rate * 0.3 + recent_success_rate * 0.2 + usage_frequency * 0.1
    ///        + freshness * 0.15 + user_approval * 0.15 + command_validity * 0.1
    pub fn overall_score(&self) -> f64 {
        self.success_rate * 0.3
            + self.recent_success_rate * 0.2
            + self.usage_frequency * 0.1
            + self.freshness * 0.15
            + self.user_approval * 0.15
            + self.command_validity * 0.1
    }
}

/// Health report for a single skill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillHealthReport {
    /// Skill name.
    pub skill_name: String,
    /// Overall health score (0.0-1.0).
    pub health_score: f64,
    /// Health status classification.
    pub status: HealthStatus,
    /// Detailed breakdown of score components.
    pub breakdown: HealthBreakdown,
    /// Human-readable recommendations based on health analysis.
    pub recommendations: Vec<String>,
    /// When this report was generated.
    pub analyzed_at: DateTime<Utc>,
}

/// Monitors skill health by analyzing telemetry data.
pub struct SkillHealthMonitor {
    telemetry_store: SkillTelemetryStore,
}

impl SkillHealthMonitor {
    /// Create a new health monitor.
    pub fn new(store: Arc<dyn Store>) -> Self {
        let telemetry_store = SkillTelemetryStore::new(store);
        Self { telemetry_store }
    }

    /// Analyze health for a single skill.
    pub async fn analyze_skill(&self, skill_name: &str) -> Result<Option<SkillHealthReport>> {
        let telemetry = match self.telemetry_store.get_telemetry(skill_name).await? {
            Some(t) => t,
            None => return Ok(None),
        };

        let breakdown = self.compute_breakdown(&telemetry);
        let health_score = breakdown.overall_score();
        let status = HealthStatus::from_score(health_score);
        let recommendations = self.generate_recommendations(&telemetry, &breakdown, status);

        Ok(Some(SkillHealthReport {
            skill_name: skill_name.to_string(),
            health_score,
            status,
            breakdown,
            recommendations,
            analyzed_at: Utc::now(),
        }))
    }

    /// Analyze health for all skills and return a summary.
    pub async fn analyze_all_skills(&self) -> Result<Vec<SkillHealthReport>> {
        let all_telemetry = self.telemetry_store.list_all().await?;
        let mut reports = Vec::new();

        for telemetry in all_telemetry {
            let breakdown = self.compute_breakdown(&telemetry);
            let health_score = breakdown.overall_score();
            let status = HealthStatus::from_score(health_score);
            let recommendations = self.generate_recommendations(&telemetry, &breakdown, status);

            reports.push(SkillHealthReport {
                skill_name: telemetry.skill_name.clone(),
                health_score,
                status,
                breakdown,
                recommendations,
                analyzed_at: Utc::now(),
            });
        }

        Ok(reports)
    }

    /// Record a health check in the change log.
    pub fn record_health_check(
        &self,
        report: &SkillHealthReport,
        change_log: &dyn ChangeLog,
    ) -> Result<()> {
        let entry =
            ChangeEntryBuilder::new(EntityType::Skill, &report.skill_name, ChangeType::Update)
                .reason(format!(
                    "Health check: {} (score: {:.2})",
                    report.status.description(),
                    report.health_score
                ))
                .trigger("skill_health_monitor".to_string())
                .build(change_log);
        change_log.record(entry)?;
        Ok(())
    }

    /// Compute health breakdown from telemetry data.
    fn compute_breakdown(&self, telemetry: &SkillTelemetry) -> HealthBreakdown {
        let success_rate = if telemetry.activation_count > 0 {
            telemetry.success_count as f64 / telemetry.activation_count as f64
        } else {
            0.0
        };

        let recent_success_rate = telemetry.recent_success_rate();

        // Normalize usage frequency: assume max 1000 activations = 1.0
        let usage_frequency = (telemetry.activation_count as f64 / 1000.0).min(1.0);

        // Freshness: days since last use, capped at 90 days
        let now = Utc::now().timestamp_millis() as u64;
        let days_since_last_use = if telemetry.last_used > 0 {
            (now.saturating_sub(telemetry.last_used)) / (24 * 60 * 60 * 1000)
        } else {
            90
        };
        let freshness = 1.0 - (days_since_last_use as f64 / 90.0).min(1.0);

        // User approval: inverse of failure rate
        let user_approval = if telemetry.activation_count > 0 {
            1.0 - (telemetry.failure_count as f64 / telemetry.activation_count as f64)
        } else {
            0.0
        };

        // Command validity: absence of critical failures
        // If there are any failures with "critical" or "fatal" keywords, reduce score
        let has_critical_failures = telemetry.common_failures.iter().any(|f| {
            let snippet_lower = f.error_snippet.to_lowercase();
            snippet_lower.contains("critical") || snippet_lower.contains("fatal")
        });
        let command_validity = if has_critical_failures { 0.3 } else { 1.0 };

        HealthBreakdown {
            success_rate,
            recent_success_rate,
            usage_frequency,
            freshness,
            user_approval,
            command_validity,
        }
    }

    /// Generate recommendations based on health analysis.
    fn generate_recommendations(
        &self,
        telemetry: &SkillTelemetry,
        breakdown: &HealthBreakdown,
        status: HealthStatus,
    ) -> Vec<String> {
        let mut recommendations = Vec::new();

        match status {
            HealthStatus::Healthy => {
                // No recommendations needed for healthy skills
            }
            HealthStatus::NeedsAttention => {
                if breakdown.success_rate < 0.7 {
                    recommendations.push(format!(
                        "Success rate is {:.1}%. Review common failures and consider patching.",
                        breakdown.success_rate * 100.0
                    ));
                }
                if breakdown.freshness < 0.5 {
                    recommendations.push(
                        "Skill has not been used recently. Consider if it's still relevant."
                            .to_string(),
                    );
                }
                if breakdown.user_approval < 0.7 {
                    recommendations.push(
                        "High failure rate suggests user dissatisfaction. Review error patterns."
                            .to_string(),
                    );
                }
            }
            HealthStatus::Unhealthy => {
                if breakdown.success_rate < 0.5 {
                    recommendations.push(format!(
                        "Critical: Success rate is only {:.1}%. Skill may need major revision or deprecation.",
                        breakdown.success_rate * 100.0
                    ));
                }
                if breakdown.recent_success_rate < 0.5 {
                    recommendations.push(format!(
                        "Recent success rate is {:.1}%. Skill is performing poorly in recent usage.",
                        breakdown.recent_success_rate * 100.0
                    ));
                }
                if !telemetry.common_failures.is_empty() {
                    let top_failure = &telemetry.common_failures[0];
                    recommendations.push(format!(
                        "Most common failure: '{}' (occurred {} times). Consider adding error handling.",
                        top_failure.error_snippet, top_failure.count
                    ));
                }
                if breakdown.freshness < 0.3 {
                    recommendations.push(
                        "Skill is rarely used and may be obsolete. Consider archiving.".to_string(),
                    );
                }
                recommendations.push(
                    "Consider deprecating this skill or merging with a similar skill.".to_string(),
                );
            }
        }

        recommendations
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_state::memory::store::InMemoryStore;
    use echo_state::skill_telemetry::SkillExecutionRecord;

    fn make_record(skill_name: &str, success: bool, error: Option<&str>) -> SkillExecutionRecord {
        SkillExecutionRecord {
            skill_name: skill_name.to_string(),
            session_id: "test-session".to_string(),
            activated_at: Utc::now().timestamp_millis() as u64,
            duration_ms: 1000,
            tools_used: vec!["Bash".to_string()],
            tool_calls_count: 1,
            success,
            error_message: error.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn test_healthy_skill() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let monitor = SkillHealthMonitor::new(store.clone());

        // Record 10 successful executions
        for _ in 0..10 {
            let telemetry_store = SkillTelemetryStore::new(store.clone());
            telemetry_store
                .record_execution(&make_record("test-skill", true, None))
                .await
                .unwrap();
        }

        let report = monitor.analyze_skill("test-skill").await.unwrap().unwrap();
        assert_eq!(report.status, HealthStatus::Healthy);
        assert!(report.health_score >= 0.7);
        assert!(report.recommendations.is_empty());
    }

    #[tokio::test]
    async fn test_unhealthy_skill() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let monitor = SkillHealthMonitor::new(store.clone());

        // Record 8 failures out of 10 executions
        let telemetry_store = SkillTelemetryStore::new(store.clone());
        for _ in 0..2 {
            telemetry_store
                .record_execution(&make_record("test-skill", true, None))
                .await
                .unwrap();
        }
        for _ in 0..8 {
            telemetry_store
                .record_execution(&make_record("test-skill", false, Some("permission denied")))
                .await
                .unwrap();
        }

        let report = monitor.analyze_skill("test-skill").await.unwrap().unwrap();
        assert_eq!(report.status, HealthStatus::Unhealthy);
        assert!(report.health_score < 0.4);
        assert!(!report.recommendations.is_empty());
    }

    #[tokio::test]
    async fn test_needs_attention_skill() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let monitor = SkillHealthMonitor::new(store.clone());

        // Record 5 successes and 5 failures (50% success rate)
        let telemetry_store = SkillTelemetryStore::new(store.clone());
        for _ in 0..5 {
            telemetry_store
                .record_execution(&make_record("test-skill", true, None))
                .await
                .unwrap();
        }
        for _ in 0..5 {
            telemetry_store
                .record_execution(&make_record("test-skill", false, Some("timeout")))
                .await
                .unwrap();
        }

        let report = monitor.analyze_skill("test-skill").await.unwrap().unwrap();
        assert_eq!(report.status, HealthStatus::NeedsAttention);
        assert!(report.health_score >= 0.4 && report.health_score < 0.7);
    }

    #[test]
    fn test_health_status_from_score() {
        assert_eq!(HealthStatus::from_score(0.8), HealthStatus::Healthy);
        assert_eq!(HealthStatus::from_score(0.7), HealthStatus::Healthy);
        assert_eq!(HealthStatus::from_score(0.5), HealthStatus::NeedsAttention);
        assert_eq!(HealthStatus::from_score(0.4), HealthStatus::NeedsAttention);
        assert_eq!(HealthStatus::from_score(0.3), HealthStatus::Unhealthy);
        assert_eq!(HealthStatus::from_score(0.0), HealthStatus::Unhealthy);
    }

    #[test]
    fn test_health_breakdown_calculation() {
        let breakdown = HealthBreakdown {
            success_rate: 0.9,
            recent_success_rate: 0.95,
            usage_frequency: 0.5,
            freshness: 0.8,
            user_approval: 0.9,
            command_validity: 1.0,
        };

        let score = breakdown.overall_score();
        // 0.9*0.3 + 0.95*0.2 + 0.5*0.1 + 0.8*0.15 + 0.9*0.15 + 1.0*0.1
        // = 0.27 + 0.19 + 0.05 + 0.12 + 0.135 + 0.1 = 0.865
        assert!((score - 0.865).abs() < 0.01);
    }

    #[test]
    fn test_critical_failure_reduces_validity() {
        let store = Arc::new(InMemoryStore::new()) as Arc<dyn Store>;
        let monitor = SkillHealthMonitor::new(store.clone());

        let mut telemetry = SkillTelemetry::new("test-skill");
        telemetry.activation_count = 10;
        telemetry.success_count = 9;
        telemetry.failure_count = 1;
        telemetry
            .common_failures
            .push(echo_state::skill_telemetry::FailurePattern {
                error_snippet: "CRITICAL: system failure".to_string(),
                count: 1,
                last_occurred: Utc::now().timestamp_millis() as u64,
            });

        let breakdown = monitor.compute_breakdown(&telemetry);
        assert_eq!(breakdown.command_validity, 0.3);
    }
}
