//! Notebook tracker — records agent analysis steps for reproducibility
//!
//! Each step (tool invocation) is recorded as a `NotebookCell`. The tracker
//! can export the full session as Markdown or JSON for reproducibility and
//! sharing, similar to Jupyter notebooks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// A single recorded step in the analysis notebook.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookCell {
    /// Step index (0-based, sequential).
    pub step_index: usize,
    /// Tool name that was invoked.
    pub tool_name: String,
    /// Short summary of the input parameters.
    pub input_summary: String,
    /// Short summary of the output (truncated to ~200 chars).
    pub output_summary: String,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Timestamp when this step was recorded.
    pub timestamp: DateTime<Utc>,
}

/// Thread-safe notebook tracker that records analysis steps.
#[derive(Debug, Clone)]
pub struct NotebookTracker {
    cells: Arc<RwLock<Vec<NotebookCell>>>,
}

impl NotebookTracker {
    /// Create a new empty tracker.
    pub fn new() -> Self {
        Self {
            cells: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Record a new cell in the notebook.
    pub fn record_cell(
        &self,
        tool_name: String,
        input_summary: String,
        output_summary: String,
        duration_ms: u64,
    ) {
        let mut cells = self.cells.write().unwrap();
        let step_index = cells.len();
        cells.push(NotebookCell {
            step_index,
            tool_name,
            input_summary: input_summary.chars().take(200).collect(),
            output_summary: output_summary.chars().take(200).collect(),
            duration_ms,
            timestamp: Utc::now(),
        });
    }

    /// Get all recorded cells.
    pub fn cells(&self) -> Vec<NotebookCell> {
        self.cells.read().unwrap().clone()
    }

    /// Export the notebook as Markdown.
    pub fn export_markdown(&self) -> String {
        let cells = self.cells.read().unwrap();
        let mut md = String::from("# Analysis Notebook\n\n");
        for cell in cells.iter() {
            md.push_str(&format!(
                "## Step {}: `{}`\n\n- **Input**: {}\n- **Output**: {}\n- **Duration**: {}ms\n- **Time**: {}\n\n",
                cell.step_index, cell.tool_name, cell.input_summary,
                cell.output_summary, cell.duration_ms, cell.timestamp
            ));
        }
        md
    }

    /// Export the notebook as JSON.
    pub fn export_json(&self) -> String {
        let cells = self.cells.read().unwrap();
        serde_json::to_string_pretty(&*cells).unwrap_or_else(|_| "[]".to_string())
    }

    /// Get the number of recorded steps.
    pub fn len(&self) -> usize {
        self.cells.read().unwrap().len()
    }

    /// Check if the notebook is empty.
    pub fn is_empty(&self) -> bool {
        self.cells.read().unwrap().is_empty()
    }
}

impl Default for NotebookTracker {
    fn default() -> Self {
        Self::new()
    }
}