//! Task verification system
//!
//! This module provides the `Verifier` trait and implementations for
//! verifying task completion. Tasks cannot be marked as done without
//! passing verification.

use crate::tasks::{Task, VerificationResult, VerificationType};
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

// ── Verifier Trait ──────────────────────────────────────────────────────────

/// Trait for task verification
///
/// Implementations verify whether a task has been completed successfully
/// based on the verification specification.
#[async_trait]
pub trait Verifier: Send + Sync {
    /// Verify task completion
    ///
    /// # Arguments
    /// * `task` - The task to verify
    ///
    /// # Returns
    /// * `Ok(VerificationResult)` - Verification result (passed/failed)
    /// * `Err(String)` - Verification error (e.g., timeout, execution error)
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String>;

    /// Get verifier name
    fn name(&self) -> &str;
}

// ── CommandVerifier ─────────────────────────────────────────────────────────

/// Verifier that runs a shell command to verify task completion
pub struct CommandVerifier {
    timeout_secs: u64,
}

impl CommandVerifier {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Run a shell command and capture output
    async fn run_command(&self, command: &str) -> Result<(i32, String, String), String> {
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        let output = timeout(
            Duration::from_secs(self.timeout_secs),
            Command::new("sh").arg("-c").arg(command).output(),
        )
        .await
        .map_err(|_| format!("Command timed out after {}s", self.timeout_secs))?
        .map_err(|e| format!("Failed to execute command: {}", e))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok((exit_code, stdout, stderr))
    }
}

#[async_trait]
impl Verifier for CommandVerifier {
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String> {
        let command = task
            .verification
            .command
            .as_ref()
            .ok_or("Command verification requires a command")?;

        let start = std::time::Instant::now();
        let (exit_code, stdout, stderr) = self.run_command(command).await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        let expected = task.verification.expected.as_deref();
        let passed = if let Some(expected_exit) = expected {
            // Parse expected exit code
            if let Ok(expected_code) = expected_exit.parse::<i32>() {
                exit_code == expected_code
            } else {
                // Check if stdout contains expected string
                stdout.contains(expected_exit)
            }
        } else {
            // Default: success if exit code is 0
            exit_code == 0
        };

        Ok(VerificationResult {
            verification_type: VerificationType::Command,
            passed,
            output: format!(
                "Exit code: {}\nStdout: {}\nStderr: {}",
                exit_code, stdout, stderr
            ),
            duration_ms,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "command"
    }
}

// ── FileExistsVerifier ──────────────────────────────────────────────────────

/// Verifier that checks if files exist
pub struct FileExistsVerifier;

#[async_trait]
impl Verifier for FileExistsVerifier {
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String> {
        let start = std::time::Instant::now();

        // Check if expected field contains file paths
        let expected = task
            .verification
            .expected
            .as_ref()
            .ok_or("FileExists verification requires expected file paths")?;

        // Parse file paths (comma-separated)
        let paths: Vec<&str> = expected.split(',').map(|s| s.trim()).collect();
        let mut missing = Vec::new();
        let mut found = Vec::new();

        for path in paths {
            if Path::new(path).exists() {
                found.push(path);
            } else {
                missing.push(path);
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        let passed = missing.is_empty();

        Ok(VerificationResult {
            verification_type: VerificationType::FileExists,
            passed,
            output: format!("Found: {:?}\nMissing: {:?}", found, missing),
            duration_ms,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "file_exists"
    }
}

// ── DiffCheckVerifier ───────────────────────────────────────────────────────

/// Verifier that checks if files have been modified
pub struct DiffCheckVerifier {
    timeout_secs: u64,
}

impl DiffCheckVerifier {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Run git diff on a file
    async fn git_diff(&self, path: &str) -> Result<String, String> {
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        let output = timeout(
            Duration::from_secs(self.timeout_secs),
            Command::new("git")
                .args(&["diff", "--name-only", path])
                .output(),
        )
        .await
        .map_err(|_| format!("Git diff timed out after {}s", self.timeout_secs))?
        .map_err(|e| format!("Failed to execute git diff: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    }
}

#[async_trait]
impl Verifier for DiffCheckVerifier {
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String> {
        let start = std::time::Instant::now();

        // Check if expected field contains file paths
        let expected = task
            .verification
            .expected
            .as_ref()
            .ok_or("DiffCheck verification requires expected file paths")?;

        // Parse file paths (comma-separated)
        let paths: Vec<&str> = expected.split(',').map(|s| s.trim()).collect();
        let mut modified = Vec::new();
        let mut unchanged = Vec::new();

        for path in paths {
            let diff_output = self.git_diff(path).await?;
            if diff_output.trim().is_empty() {
                unchanged.push(path);
            } else {
                modified.push(path);
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        let passed = !modified.is_empty();

        Ok(VerificationResult {
            verification_type: VerificationType::DiffCheck,
            passed,
            output: format!("Modified: {:?}\nUnchanged: {:?}", modified, unchanged),
            duration_ms,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "diff_check"
    }
}

// ── TestVerifier ────────────────────────────────────────────────────────────

/// Verifier that runs tests to verify task completion
pub struct TestVerifier {
    timeout_secs: u64,
}

impl TestVerifier {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Run tests and capture output
    async fn run_tests(&self, command: &str) -> Result<(i32, String, String), String> {
        use tokio::process::Command;
        use tokio::time::{Duration, timeout};

        let output = timeout(
            Duration::from_secs(self.timeout_secs),
            Command::new("sh").arg("-c").arg(command).output(),
        )
        .await
        .map_err(|_| format!("Test command timed out after {}s", self.timeout_secs))?
        .map_err(|e| format!("Failed to execute test command: {}", e))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok((exit_code, stdout, stderr))
    }
}

#[async_trait]
impl Verifier for TestVerifier {
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String> {
        let command = task
            .verification
            .command
            .as_ref()
            .ok_or("Test verification requires a test command")?;

        let start = std::time::Instant::now();
        let (exit_code, stdout, stderr) = self.run_tests(command).await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Tests pass if exit code is 0
        let passed = exit_code == 0;

        Ok(VerificationResult {
            verification_type: VerificationType::Test,
            passed,
            output: format!(
                "Exit code: {}\nStdout: {}\nStderr: {}",
                exit_code, stdout, stderr
            ),
            duration_ms,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "test"
    }
}

// ── HumanReviewVerifier ─────────────────────────────────────────────────────

/// Verifier that requires human review
pub struct HumanReviewVerifier;

#[async_trait]
impl Verifier for HumanReviewVerifier {
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String> {
        let start = std::time::Instant::now();

        // For human review, we check if the task has been approved
        // This would typically be set by an external system
        let passed = task
            .verification_result
            .as_ref()
            .map(|r| r.passed)
            .unwrap_or(false);

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(VerificationResult {
            verification_type: VerificationType::HumanReview,
            passed,
            output: if passed {
                "Human review approved".to_string()
            } else {
                "Awaiting human review".to_string()
            },
            duration_ms,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "human_review"
    }
}

// ── LlmReviewVerifier ───────────────────────────────────────────────────────

/// Verifier that uses LLM to review task completion
pub struct LlmReviewVerifier {
    timeout_secs: u64,
}

impl LlmReviewVerifier {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Use LLM to review task completion
    async fn llm_review(&self, task: &Task) -> Result<(bool, String), String> {
        // This would typically call an LLM API
        // For now, we'll simulate a basic review
        use tokio::time::{Duration, timeout};

        let review = timeout(Duration::from_secs(self.timeout_secs), async {
            // Simulate LLM review
            let task_desc = &task.description;
            let result = task.result.as_deref().unwrap_or("No result");

            // Simple heuristic: check if result is non-empty and mentions key aspects
            let has_result = !result.trim().is_empty();
            let mentions_task = result.to_lowercase().contains(
                &task_desc
                    .to_lowercase()
                    .chars()
                    .take(20)
                    .collect::<String>(),
            );

            let passed = has_result && mentions_task;
            let output = format!(
                "LLM Review:\n- Has result: {}\n- Mentions task: {}\n- Passed: {}",
                has_result, mentions_task, passed
            );

            (passed, output)
        })
        .await
        .map_err(|_| format!("LLM review timed out after {}s", self.timeout_secs))?;

        Ok(review)
    }
}

#[async_trait]
impl Verifier for LlmReviewVerifier {
    async fn verify(&self, task: &Task) -> Result<VerificationResult, String> {
        let start = std::time::Instant::now();
        let (passed, output) = self.llm_review(task).await?;
        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(VerificationResult {
            verification_type: VerificationType::LlmReview,
            passed,
            output,
            duration_ms,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "llm_review"
    }
}

// ── VerifierFactory ─────────────────────────────────────────────────────────

/// Factory for creating verifiers based on verification type
pub struct VerifierFactory;

impl VerifierFactory {
    /// Create a verifier based on verification type
    pub fn create(verification_type: &VerificationType, timeout_secs: u64) -> Arc<dyn Verifier> {
        match verification_type {
            VerificationType::Command => Arc::new(CommandVerifier::new(timeout_secs)),
            VerificationType::FileExists => Arc::new(FileExistsVerifier),
            VerificationType::DiffCheck => Arc::new(DiffCheckVerifier::new(timeout_secs)),
            VerificationType::Test => Arc::new(TestVerifier::new(timeout_secs)),
            VerificationType::HumanReview => Arc::new(HumanReviewVerifier),
            VerificationType::LlmReview => Arc::new(LlmReviewVerifier::new(timeout_secs)),
            VerificationType::None => Arc::new(NoopVerifier),
        }
    }
}

// ── NoopVerifier ────────────────────────────────────────────────────────────

/// No-op verifier that always passes
pub struct NoopVerifier;

#[async_trait]
impl Verifier for NoopVerifier {
    async fn verify(&self, _task: &Task) -> Result<VerificationResult, String> {
        Ok(VerificationResult {
            verification_type: VerificationType::None,
            passed: true,
            output: "No verification required".to_string(),
            duration_ms: 0,
            retry_count: 0,
        })
    }

    fn name(&self) -> &str {
        "none"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{FallbackStrategy, Task, VerificationSpec, VerificationType};

    #[tokio::test]
    async fn test_command_verifier_success() {
        let verifier = CommandVerifier::new(10);
        let mut task = Task::new("test-task", "Test task");
        task.verification = VerificationSpec {
            verification_type: VerificationType::Command,
            command: Some("echo 'success'".to_string()),
            expected: Some("0".to_string()),
            timeout_secs: 10,
            retry_count: 0,
            fallback_on_failure: FallbackStrategy::Abort,
        };

        let result = verifier.verify(&task).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.passed);
        assert_eq!(result.verification_type, VerificationType::Command);
    }

    #[tokio::test]
    async fn test_command_verifier_failure() {
        let verifier = CommandVerifier::new(10);
        let mut task = Task::new("test-task", "Test task");
        task.verification = VerificationSpec {
            verification_type: VerificationType::Command,
            command: Some("exit 1".to_string()),
            expected: Some("0".to_string()),
            timeout_secs: 10,
            retry_count: 0,
            fallback_on_failure: FallbackStrategy::Abort,
        };

        let result = verifier.verify(&task).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(!result.passed);
    }

    #[tokio::test]
    async fn test_file_exists_verifier() {
        let verifier = FileExistsVerifier;
        let mut task = Task::new("test-task", "Test task");
        task.verification = VerificationSpec {
            verification_type: VerificationType::FileExists,
            command: None,
            expected: Some("/tmp".to_string()),
            timeout_secs: 10,
            retry_count: 0,
            fallback_on_failure: FallbackStrategy::Abort,
        };

        let result = verifier.verify(&task).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.passed);
        assert_eq!(result.verification_type, VerificationType::FileExists);
    }

    #[tokio::test]
    async fn test_noop_verifier() {
        let verifier = NoopVerifier;
        let task = Task::new("test-task", "Test task");

        let result = verifier.verify(&task).await;
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.passed);
        assert_eq!(result.verification_type, VerificationType::None);
    }

    #[test]
    fn test_verifier_factory() {
        let verifier = VerifierFactory::create(&VerificationType::Command, 10);
        assert_eq!(verifier.name(), "command");

        let verifier = VerifierFactory::create(&VerificationType::FileExists, 10);
        assert_eq!(verifier.name(), "file_exists");

        let verifier = VerifierFactory::create(&VerificationType::None, 10);
        assert_eq!(verifier.name(), "none");
    }
}
