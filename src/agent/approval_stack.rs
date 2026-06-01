//! Session-level approval decision stack.
//!
//! Tracks once/always/deny decisions per tool+path combination within a session.
//! This enables "allow always for this session" and "deny this path" patterns.

use std::collections::HashMap;
use std::sync::Mutex;

/// A single permission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Allow this specific call once.
    Once,
    /// Allow all calls to this tool for this session.
    Always,
    /// Deny this specific call.
    Deny,
    /// Deny all calls to this tool for this session.
    DenyAlways,
}

/// Session-scoped approval stack.
pub struct ApprovalStack {
    /// tool_name -> (path_pattern -> decision)
    decisions: Mutex<HashMap<String, HashMap<String, ApprovalDecision>>>,
}

impl ApprovalStack {
    pub fn new() -> Self {
        Self {
            decisions: Mutex::new(HashMap::new()),
        }
    }

    /// Check if a decision has already been made for this tool+path.
    pub fn check(&self, tool_name: &str, path: &str) -> Option<ApprovalDecision> {
        let map = self.decisions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tool_map) = map.get(tool_name) {
            // Exact path match
            if let Some(&dec) = tool_map.get(path) {
                return Some(dec);
            }
            // "always" / "deny_always" catch-all
            if let Some(&dec) = tool_map.get("*") {
                return Some(dec);
            }
        }
        None
    }

    /// Record a decision for a tool+path.
    pub fn record(&self, tool_name: &str, path: &str, decision: ApprovalDecision) {
        let mut map = self.decisions.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(tool_name.to_string())
            .or_default()
            .insert(path.to_string(), decision);
    }

    /// Clear all decisions (e.g., on session reset).
    pub fn clear(&self) {
        self.decisions.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

impl Default for ApprovalStack {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_once_then_check() {
        let stack = ApprovalStack::new();
        assert!(stack.check("write_file", "src/main.rs").is_none());
        stack.record("write_file", "src/main.rs", ApprovalDecision::Once);
        assert_eq!(
            stack.check("write_file", "src/main.rs"),
            Some(ApprovalDecision::Once)
        );
        // Different path not affected
        assert!(stack.check("write_file", "src/lib.rs").is_none());
    }

    #[test]
    fn test_always_catch_all() {
        let stack = ApprovalStack::new();
        stack.record("read_file", "*", ApprovalDecision::Always);
        assert_eq!(
            stack.check("read_file", "any/path.rs"),
            Some(ApprovalDecision::Always)
        );
    }
}
