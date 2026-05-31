//! Loop detection for agent runs.
//!
//! Detects three types of loops:
//! 1. Exact duplicate: same (tool_name, args) repeated N times
//! 2. Same-tool failure: same tool fails N times consecutively
//! 3. No-progress: N iterations without any file writes or task completions

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopDetectorConfig {
    /// Max identical tool calls before warning
    pub exact_threshold: usize,
    /// Max consecutive failures of same tool
    pub failure_threshold: usize,
    /// Max iterations without progress (file write, task update)
    pub no_progress_threshold: usize,
}

impl Default for LoopDetectorConfig {
    fn default() -> Self {
        Self {
            exact_threshold: 3,
            failure_threshold: 3,
            no_progress_threshold: 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopVerdict {
    /// Continue normally
    Continue,
    /// Warn the agent (inject system message)
    Warn(String),
    /// Force stop the agent loop
    Break(String),
}

pub struct LoopDetector {
    config: LoopDetectorConfig,
    /// (tool_name, args_json_string) -> count
    exact_history: HashMap<(String, String), usize>,
    /// tool_name -> consecutive failure count
    failure_streak: HashMap<String, usize>,
    /// Iterations since last progress
    iterations_without_progress: usize,
    /// Tools that count as "progress"
    progress_tools: HashSet<String>,
    /// Last tool call for streak detection
    #[allow(dead_code)]
    last_tool: Option<(String, String)>,
}

impl LoopDetector {
    pub fn new(config: LoopDetectorConfig) -> Self {
        let mut progress_tools = HashSet::new();
        for tool in &[
            "edit_file",
            "write_file",
            "create_file",
            "delete_file",
            "create_task",
            "update_task",
            "git_commit",
            "shell",
        ] {
            progress_tools.insert(tool.to_string());
        }
        Self {
            config,
            exact_history: HashMap::new(),
            failure_streak: HashMap::new(),
            iterations_without_progress: 0,
            progress_tools,
            last_tool: None,
        }
    }

    /// Record a tool call and its outcome.
    pub fn record_tool_call(&mut self, name: &str, args_json: &str, success: bool) {
        let key = (name.to_string(), args_json.to_string());

        // Track exact duplicates
        let count = self.exact_history.entry(key.clone()).or_insert(0);
        *count += 1;

        // Track failure streaks
        if success {
            self.failure_streak.remove(name);
            // Reset failure streak for this tool
        } else {
            let streak = self.failure_streak.entry(name.to_string()).or_insert(0);
            *streak += 1;
        }

        // Track progress
        if self.progress_tools.contains(name) && success {
            self.iterations_without_progress = 0;
        }

        self.last_tool = Some(key);
    }

    /// Record an iteration (called once per agent loop iteration).
    pub fn record_iteration(&mut self) {
        self.iterations_without_progress += 1;
    }

    /// Check for loops and return a verdict.
    pub fn check(&self) -> LoopVerdict {
        // Check exact duplicates
        for ((name, _args), count) in &self.exact_history {
            if *count >= self.config.exact_threshold {
                return LoopVerdict::Break(format!(
                    "Loop detected: tool '{}' called with identical arguments {} times. Stopping to prevent runaway execution.",
                    name, count
                ));
            }
        }

        // Check failure streaks
        for (name, streak) in &self.failure_streak {
            if *streak >= self.config.failure_threshold {
                return LoopVerdict::Warn(format!(
                    "Tool '{}' has failed {} times consecutively. Consider a different approach or check for issues.",
                    name, streak
                ));
            }
        }

        // Check no-progress
        if self.iterations_without_progress >= self.config.no_progress_threshold {
            return LoopVerdict::Warn(format!(
                "No progress in {} iterations (no file writes or task updates). Consider whether the task is achievable or if you should report back to the user.",
                self.iterations_without_progress
            ));
        }

        LoopVerdict::Continue
    }

    /// Reset all tracking state (e.g., when starting a new task).
    pub fn reset(&mut self) {
        self.exact_history.clear();
        self.failure_streak.clear();
        self.iterations_without_progress = 0;
        self.last_tool = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_loop() {
        let mut detector = LoopDetector::new(LoopDetectorConfig::default());
        detector.record_tool_call("read_file", r#"{"path":"a"}"#, true);
        detector.record_tool_call("read_file", r#"{"path":"b"}"#, true);
        detector.record_tool_call("edit_file", r#"{"path":"a"}"#, true);
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }

    #[test]
    fn test_exact_loop() {
        let mut detector = LoopDetector::new(LoopDetectorConfig::default());
        for _ in 0..3 {
            detector.record_tool_call("read_file", r#"{"path":"a"}"#, true);
        }
        match detector.check() {
            LoopVerdict::Break(msg) => assert!(msg.contains("Loop detected")),
            other => panic!("Expected Break, got {:?}", other),
        }
    }

    #[test]
    fn test_failure_streak() {
        let mut detector = LoopDetector::new(LoopDetectorConfig::default());
        for _ in 0..3 {
            detector.record_tool_call("shell", "bad_cmd", false);
        }
        match detector.check() {
            LoopVerdict::Warn(msg) => assert!(msg.contains("failed 3 times")),
            other => panic!("Expected Warn, got {:?}", other),
        }
    }

    #[test]
    fn test_no_progress() {
        let mut detector = LoopDetector::new(LoopDetectorConfig::default());
        for i in 0..8 {
            detector.record_tool_call("read_file", &format!(r#"{{"path":"{}"}}"#, i), true);
            detector.record_iteration();
        }
        match detector.check() {
            LoopVerdict::Warn(msg) => assert!(msg.contains("No progress")),
            other => panic!("Expected Warn, got {:?}", other),
        }
    }

    #[test]
    fn test_progress_resets_counter() {
        let mut detector = LoopDetector::new(LoopDetectorConfig::default());
        for i in 0..7 {
            detector.record_tool_call("read_file", &format!(r#"{{"path":"{}"}}"#, i), true);
            detector.record_iteration();
        }
        detector.record_tool_call("write_file", r#"{"path":"x"}"#, true);
        detector.record_iteration();
        // Should be fine now — progress was made
        assert_eq!(detector.check(), LoopVerdict::Continue);
    }
}
