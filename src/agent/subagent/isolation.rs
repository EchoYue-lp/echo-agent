//! Product-neutral isolation boundary for Fork-dispatched subagents.

use std::path::PathBuf;
use std::sync::Arc;

use super::types::{ObservedIsolation, SubagentArtifact, SubagentEvidence};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationRequest {
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Default)]
pub struct IsolationOutcome {
    pub summary: String,
    pub artifacts: Vec<SubagentArtifact>,
    pub evidence: Vec<SubagentEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsolationError {
    pub message: String,
}

impl std::fmt::Display for IsolationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IsolationError {}

impl IsolationError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub struct IsolationHandle {
    pub path: PathBuf,
    pub observed: ObservedIsolation,
    pub finalize: Box<dyn FnOnce() -> Result<IsolationOutcome, IsolationError> + Send>,
}

impl std::fmt::Debug for IsolationHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IsolationHandle")
            .field("path", &self.path)
            .field("observed", &self.observed)
            .finish_non_exhaustive()
    }
}

/// Application-provided isolation strategy. The framework neither interprets
/// `kind` nor owns the lifecycle policy behind the returned handle.
pub trait IsolationProvider: Send + Sync {
    fn isolate(&self, request: &IsolationRequest) -> Result<IsolationHandle, IsolationError>;
}

pub type SharedIsolationProvider = Arc<dyn IsolationProvider>;
