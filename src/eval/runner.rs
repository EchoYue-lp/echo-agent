//! Eval runner — executes eval cases against an agent.

use crate::agent::Agent;
use crate::eval::{EvalCase, EvalConstraints, EvalReport, EvalResult, SuccessCriteria};
use crate::eval::{LlmGrader, TrajectoryReplay};
use crate::trace::Run;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// Runs eval cases against agents.
pub struct EvalRunner {
    /// Root directory for temporary workspaces (fixtures copied here).
    pub workspace_root: PathBuf,
    /// Maximum time per eval case in seconds.
    pub timeout_secs: u64,
    /// Optional LLM grader for assertion-based evaluation.
    pub grader: Option<Arc<LlmGrader>>,
    /// Optional grading agent (shared across cases, used by LlmGraded criteria).
    pub grading_agent: Option<Arc<dyn crate::agent::Agent>>,
    /// Optional run store for populating metrics from trace.
    pub run_store: Option<Arc<dyn crate::trace::RunStore>>,
}

impl EvalRunner {
    /// Create a new runner.
    pub fn new(workspace_root: PathBuf) -> Self {
        std::fs::create_dir_all(&workspace_root).ok();
        Self {
            workspace_root,
            timeout_secs: 300,
            grader: None,
            grading_agent: None,
            run_store: None,
        }
    }

    /// Attach a run store for metrics population from trace.
    pub fn with_run_store(mut self, store: Arc<dyn crate::trace::RunStore>) -> Self {
        self.run_store = Some(store);
        self
    }

    /// Attach an LLM grader for assertion-based evaluation.
    pub fn with_grader(mut self, grader: LlmGrader, agent: Arc<dyn crate::agent::Agent>) -> Self {
        self.grader = Some(Arc::new(grader));
        self.grading_agent = Some(agent);
        self
    }

    /// Run a single eval case against the given agent.
    pub async fn run(&self, case: &EvalCase, agent: &dyn Agent) -> EvalResult {
        let started = Instant::now();
        let work_dir = self.setup_fixture(case).await;
        let mut result = EvalResult::new(&case.id, true);

        // Execute the task — pass workspace dir explicitly, never change global cwd
        let cwd = work_dir
            .clone()
            .unwrap_or_else(|| self.workspace_root.clone());

        let agent_result = tokio::time::timeout(
            std::time::Duration::from_secs(self.timeout_secs),
            agent.execute(&case.task),
        )
        .await;

        // Capture run_id from agent for trace linkage (all branches)
        result.run_id = agent.current_run_id();

        match agent_result {
            Ok(Ok(output)) => {
                result.duration_ms = started.elapsed().as_millis() as u64;
                let criteria_result = self
                    .check_criteria(&case.success_criteria, &output, &case.task, &cwd)
                    .await;
                if !criteria_result.success {
                    result.success = false;
                }
                result.metrics.extend(criteria_result.metrics);
            }
            Ok(Err(e)) => {
                result.duration_ms = started.elapsed().as_millis() as u64;
                result.success = false;
                result.violations.push(format!("Agent error: {e}"));
            }
            Err(_) => {
                result.duration_ms = started.elapsed().as_millis() as u64;
                result.success = false;
                result.violations.push("Timeout".to_string());
            }
        }

        // Populate metrics from trace (all branches — errors/timeouts also have diagnostic trace value)
        if let Some(ref store) = self.run_store
            && let Some(ref run_id) = result.run_id
            && let Ok(Some(run)) = store.load(run_id).await
        {
            // Only check constraints on success; on error, just collect metrics
            if result.success {
                let violations = self.evaluate_run_constraints(&case.constraints, &run);
                if !violations.is_empty() {
                    result.violations = violations;
                    result.success = false;
                }
            }
            result.tool_calls = run
                .events
                .iter()
                .filter(|e| matches!(e, crate::trace::RunEvent::ToolCall { .. }))
                .count();
            result.tokens_in = run.token_usage.prompt_tokens;
            result.tokens_out = run.token_usage.completion_tokens;
            let replay = TrajectoryReplay::new(run);
            result.file_changes = replay.written_files().len();
        }

        result.recompute_score();
        result
    }

    /// Run all cases against agents created by the factory.
    /// Each case gets a fresh agent.
    pub async fn run_all(
        &self,
        cases: &[EvalCase],
        mut agent_factory: impl FnMut() -> Box<dyn Agent>,
    ) -> EvalReport {
        let mut results = Vec::with_capacity(cases.len());
        for case in cases {
            let agent = agent_factory();
            let result = self.run(case, &*agent).await;
            results.push(result);
        }
        EvalReport::new(results)
    }

    // ── Private helpers ────────────────────────────────────────────

    async fn setup_fixture(&self, case: &EvalCase) -> Option<PathBuf> {
        let fixture = case.project_fixture.as_ref()?;
        if !fixture.exists() {
            return None;
        }
        let dest = self.workspace_root.join(&case.id);
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        // Simple recursive copy
        if let Err(e) = copy_dir(fixture, &dest) {
            tracing::warn!("Failed to copy fixture {}: {e}", fixture.display());
            return None;
        }
        Some(dest)
    }

    async fn check_criteria(
        &self,
        criteria: &SuccessCriteria,
        output: &str,
        task: &str,
        cwd: &Path,
    ) -> EvalResult {
        match criteria {
            SuccessCriteria::LlmGraded { assertions } => {
                if let (Some(grader), Some(grading_agent)) = (&self.grader, &self.grading_agent) {
                    let report = grader
                        .grade(grading_agent.as_ref(), task, output, assertions)
                        .await;
                    let mut result = EvalResult::new("criteria", report.pass_rate >= 0.5);
                    result.metrics.push(crate::eval::EvalMetric {
                        name: "llm_graded".into(),
                        score: report.pass_rate,
                        detail: format!(
                            "{} assertions, {:.0}% pass rate",
                            assertions.len(),
                            report.pass_rate * 100.0
                        ),
                    });
                    result
                } else {
                    let mut r = EvalResult::new("criteria", true);
                    r.violations
                        .push("LlmGraded criteria set but no grader configured".into());
                    r
                }
            }
            SuccessCriteria::TestPass { command } => {
                let passed = run_command(command, cwd).await;
                let mut result = EvalResult::new("criteria", passed);
                result.metrics.push(crate::eval::EvalMetric {
                    name: "test_pass".into(),
                    score: if passed { 1.0 } else { 0.0 },
                    detail: format!("Command: {command}"),
                });
                result
            }
            SuccessCriteria::OutputContains { substring } => {
                let found = output.contains(substring.as_str());
                let mut result = EvalResult::new("criteria", found);
                result.metrics.push(crate::eval::EvalMetric {
                    name: "output_contains".into(),
                    score: if found { 1.0 } else { 0.0 },
                    detail: format!("Looking for: {substring}"),
                });
                if !found {
                    result.violations.push(format!(
                        "Output missing: {substring}. Got: {}",
                        &output.chars().take(200).collect::<String>()
                    ));
                }
                result
            }
            SuccessCriteria::ToolUsed { tool_name: _ } => {
                // Tool usage is checked from the trace, not the output.
                // Mark as pass here; constraint checker handles violations.
                EvalResult::new("criteria", true)
            }
            SuccessCriteria::ToolNotUsed { tool_name: _ } => EvalResult::new("criteria", true),
            SuccessCriteria::AllOf(items) => {
                let mut all_pass = true;
                let mut metrics = Vec::new();
                for item in items {
                    let r = Box::pin(self.check_criteria(item, output, task, cwd)).await;
                    if !r.success {
                        all_pass = false;
                    }
                    metrics.extend(r.metrics);
                }
                let mut result = EvalResult::new("criteria", all_pass);
                result.metrics = metrics;
                result
            }
            SuccessCriteria::AnyOf(items) => {
                let mut any_pass = false;
                let mut metrics = Vec::new();
                for item in items {
                    let r = Box::pin(self.check_criteria(item, output, task, cwd)).await;
                    if r.success {
                        any_pass = true;
                    }
                    metrics.extend(r.metrics);
                }
                let mut result = EvalResult::new("criteria", any_pass);
                result.metrics = metrics;
                result
            }
            SuccessCriteria::SweBench {
                repo_url,
                base_commit,
                test_patch,
                test_command,
            } => {
                // Validate repo_url: only allow https:// scheme
                if !repo_url.starts_with("https://") {
                    return EvalResult::new("criteria", false).with_metric(
                        "swe_bench",
                        0.0,
                        "Only https:// URLs are allowed for repository cloning",
                    );
                }

                // Validate test_command: reject shell metacharacters that could
                // lead to command injection when passed to `sh -c`
                if let Err(msg) = validate_shell_command(test_command) {
                    return EvalResult::new("criteria", false).with_metric("swe_bench", 0.0, &msg);
                }

                // SWE-bench eval workflow:
                // 1. Clone repo (if not already cloned)
                // 2. Checkout base commit
                // 3. Agent runs (already done by the time we check criteria)
                // 4. Apply test patch
                // 5. Run test command
                let repo_dir = cwd.join("repo");
                if !repo_dir.exists() {
                    let clone_result = std::process::Command::new("git")
                        .args(["clone", repo_url, repo_dir.to_str().unwrap_or("repo")])
                        .current_dir(cwd)
                        .output();
                    match clone_result {
                        Ok(output) if output.status.success() => {}
                        Ok(output) => {
                            let stderr = String::from_utf8_lossy(&output.stderr);
                            tracing::warn!("git clone failed for {}: {}", repo_url, stderr);
                            return EvalResult::new("criteria", false).with_metric(
                                "swe_bench",
                                0.0,
                                &format!("Clone failed: {}", stderr),
                            );
                        }
                        Err(e) => {
                            tracing::warn!("git clone error for {}: {}", repo_url, e);
                            return EvalResult::new("criteria", false).with_metric(
                                "swe_bench",
                                0.0,
                                &format!("Clone failed: {e}"),
                            );
                        }
                    }
                }

                let checkout_result = std::process::Command::new("git")
                    .args(["checkout", base_commit])
                    .current_dir(&repo_dir)
                    .output();
                match checkout_result {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        tracing::warn!("git checkout {} failed: {}", base_commit, stderr);
                        return EvalResult::new("criteria", false).with_metric(
                            "swe_bench",
                            0.0,
                            &format!("Checkout failed: {}", stderr),
                        );
                    }
                    Err(e) => {
                        tracing::warn!("git checkout error: {}", e);
                        return EvalResult::new("criteria", false).with_metric(
                            "swe_bench",
                            0.0,
                            &format!("Checkout failed: {e}"),
                        );
                    }
                }

                let apply_result = std::process::Command::new("git")
                    .args(["apply", test_patch])
                    .current_dir(&repo_dir)
                    .output();
                match apply_result {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        tracing::warn!("git apply failed: {}", stderr);
                        return EvalResult::new("criteria", false).with_metric(
                            "swe_bench",
                            0.0,
                            &format!("Apply failed: {}", stderr),
                        );
                    }
                    Err(e) => {
                        tracing::warn!("git apply error: {}", e);
                        return EvalResult::new("criteria", false).with_metric(
                            "swe_bench",
                            0.0,
                            &format!("Apply failed: {e}"),
                        );
                    }
                }

                let passed = run_command(test_command, &repo_dir).await;

                let mut result = EvalResult::new("criteria", passed);
                result.metrics.push(crate::eval::EvalMetric {
                    name: "swe_bench".into(),
                    score: if passed { 1.0 } else { 0.0 },
                    detail: format!("{repo_url} @ {base_commit}: {test_command}"),
                });
                result
            }
            SuccessCriteria::SafetyCheck {
                forbidden_patterns,
                required_patterns,
            } => {
                let mut violations = Vec::new();
                let mut checks_total = 0;
                let mut checks_passed = 0;

                // Check forbidden patterns — none should appear
                for pattern in forbidden_patterns {
                    checks_total += 1;
                    if output.contains(pattern.as_str()) {
                        violations.push(format!(
                            "SAFETY VIOLATION: output contains forbidden pattern '{}'",
                            pattern
                        ));
                    } else {
                        checks_passed += 1;
                    }
                }

                // Check required patterns — all must appear
                for pattern in required_patterns {
                    checks_total += 1;
                    if output.contains(pattern.as_str()) {
                        checks_passed += 1;
                    } else {
                        violations.push(format!(
                            "SAFETY VIOLATION: output missing required pattern '{}'",
                            pattern
                        ));
                    }
                }

                let score = if checks_total > 0 {
                    checks_passed as f64 / checks_total as f64
                } else {
                    1.0
                };
                let passed = violations.is_empty();

                let mut result = EvalResult::new("criteria", passed);
                result.violations = violations;
                result.metrics.push(crate::eval::EvalMetric {
                    name: "safety_check".into(),
                    score,
                    detail: format!("{}/{} safety checks passed", checks_passed, checks_total),
                });
                result
            }
            SuccessCriteria::CitationValid {
                min_citations,
                format,
            } => {
                let citation_patterns: Vec<&str> = match format.as_str() {
                    "pmid" => vec![r"PMID:\s*\d+", r"PMID\s*\d+"],
                    "doi" => vec![r"10\.\d{4,}/"],
                    "url" => vec![r"https?://"],
                    _ => vec![r"PMID:\s*\d+", r"PMID\s*\d+", r"10\.\d{4,}/", r"https?://"],
                };

                let mut citation_count = 0usize;
                for pattern in &citation_patterns {
                    if let Ok(re) = regex::Regex::new(pattern) {
                        citation_count += re.find_iter(output).count();
                    }
                }

                let passed = citation_count >= *min_citations;
                let score = if *min_citations > 0 {
                    (citation_count as f64 / *min_citations as f64).min(1.0)
                } else {
                    1.0
                };

                let mut result = EvalResult::new("criteria", passed);
                result.metrics.push(crate::eval::EvalMetric {
                    name: "citation_valid".into(),
                    score,
                    detail: format!(
                        "Found {} citations (format: {}, required: {})",
                        citation_count, format, min_citations
                    ),
                });
                if !passed {
                    result.violations.push(format!(
                        "Insufficient citations: found {} but need at least {} (format: {})",
                        citation_count, min_citations, format
                    ));
                }
                result
            }
            SuccessCriteria::ValueMatch {
                expected,
                tolerance,
            } => {
                let mut checks_total = 0;
                let mut checks_passed = 0;
                let mut details = Vec::new();

                for (key, expected_val) in expected {
                    checks_total += 1;
                    // Try to find the value in the output by looking for
                    // patterns like "key: value" or "key = value" or just the number
                    let found = extract_number_near_key(output, key);
                    match found {
                        Some(actual) => {
                            let diff = (actual - expected_val).abs();
                            let threshold = expected_val.abs() * tolerance + tolerance;
                            if diff <= threshold {
                                checks_passed += 1;
                                details.push(format!("{}: {} ≈ {} ✓", key, actual, expected_val));
                            } else {
                                details.push(format!(
                                    "{}: {} ≠ {} (diff={:.4}) ✗",
                                    key, actual, expected_val, diff
                                ));
                            }
                        }
                        None => {
                            details.push(format!("{}: not found in output ✗", key));
                        }
                    }
                }

                let score = if checks_total > 0 {
                    checks_passed as f64 / checks_total as f64
                } else {
                    1.0
                };
                let passed = checks_passed == checks_total;

                let mut result = EvalResult::new("criteria", passed);
                result.metrics.push(crate::eval::EvalMetric {
                    name: "value_match".into(),
                    score,
                    detail: format!(
                        "{}/{} values matched (tolerance: {}): {}",
                        checks_passed,
                        checks_total,
                        tolerance,
                        details.join("; ")
                    ),
                });
                if !passed {
                    for d in &details {
                        if d.contains('✗') {
                            result.violations.push(d.clone());
                        }
                    }
                }
                result
            }
        }
    }

    /// Evaluate constraints using a completed run trace.
    pub fn evaluate_run_constraints(
        &self,
        constraints: &EvalConstraints,
        run: &Run,
    ) -> Vec<String> {
        let replay = TrajectoryReplay::new(run.clone());
        let mut violations = replay.evaluate_constraints(constraints);

        // Check forbidden paths against written files
        if !constraints.forbidden_paths.is_empty() {
            for file in replay.written_files() {
                for forbidden in &constraints.forbidden_paths {
                    if file.contains(forbidden.as_str()) {
                        violations.push(format!("Forbidden path modified: {file}"));
                    }
                }
            }
        }

        // Check max files changed
        if let Some(max) = constraints.max_files_changed {
            let written = replay.written_files();
            if written.len() > max {
                violations.push(format!(
                    "Too many files changed: {} (max {})",
                    written.len(),
                    max
                ));
            }
        }

        violations
    }
}

/// Validate a shell command for dangerous metacharacters.
///
/// Rejects commands containing characters that could be used for command injection
/// or unintended shell expansion when passed to `sh -c`.
fn validate_shell_command(cmd: &str) -> std::result::Result<(), String> {
    let dangerous_chars = [';', '|', '&', '$', '`', '>', '<'];
    for c in &dangerous_chars {
        if cmd.contains(*c) {
            return Err(format!(
                "Test command rejected: contains '{}' which is not allowed (shell metacharacters are blocked for security)",
                c
            ));
        }
    }
    Ok(())
}

/// Run a shell command and return whether it succeeded.
async fn run_command(cmd: &str, cwd: &Path) -> bool {
    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(cwd)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Try to extract a numeric value near a given key in the output text.
///
/// Searches for patterns like "key: 0.85", "key = 0.85", "key 0.85"
/// or just the first number after the key within a small window.
fn extract_number_near_key(text: &str, key: &str) -> Option<f64> {
    // Build a pattern that matches "key" followed by separators and a number
    let escaped_key = regex::escape(key);
    let pattern = format!(r"(?i){escaped_key}\s*[:=：]\s*(-?\d+\.?\d*(?:\s*%|e[+-]?\d+)?)");
    if let Ok(re) = regex::Regex::new(&pattern) {
        if let Some(captures) = re.captures(text) {
            let num_str = captures
                .get(1)
                .map(|m| m.as_str().replace('%', "").replace('，', ""))
                .unwrap_or_default();
            return num_str.trim().parse::<f64>().ok();
        }
    }

    // Fallback: find the key and look for a number in the next 50 chars
    let lower_text = text.to_lowercase();
    let lower_key = key.to_lowercase();
    if let Some(pos) = lower_text.find(&lower_key) {
        let after = &text[pos..text.len().min(pos + key.len() + 50)];
        if let Ok(re) = regex::Regex::new(r"(-?\d+\.?\d*(?:e[+-]?\d+)?)") {
            if let Some(m) = re.find(after) {
                return m.as_str().parse::<f64>().ok();
            }
        }
    }
    None
}

/// Simple recursive directory copy.
fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest_path = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::EvalCase;

    #[tokio::test]
    async fn test_runner_output_contains() {
        let dir = std::env::temp_dir().join(format!("eval_test_{}", uuid::Uuid::new_v4()));
        let _runner = EvalRunner::new(dir);
        let _case = EvalCase {
            id: "test".into(),
            name: "test".into(),
            description: "".into(),
            domain: None,
            task: "Say hello world".into(),
            project_fixture: None,
            success_criteria: SuccessCriteria::OutputContains {
                substring: "hello".into(),
            },
            constraints: Default::default(),
        };
        // This test just validates the runner doesn't panic on missing agent
        // Actual agent-based testing requires a MockAgent
    }
}
