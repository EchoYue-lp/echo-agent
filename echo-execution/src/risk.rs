//! Tool risk classification — per-category risk assessment for safety notices.

use echo_core::tools::ToolParameters;

/// Risk category for a tool based on its type and parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolRiskCategory {
    /// Read-only operations — no risk.
    ReadOnly,
    /// File write operations — risk: data loss or corruption.
    FileWrite,
    /// Shell/command execution — risk: arbitrary code execution.
    ShellExec,
    /// Git write operations — risk: repository damage.
    GitWrite,
    /// Database write operations — risk: data corruption.
    DatabaseWrite,
    /// Network calls — risk: data exfiltration.
    NetworkCall,
    /// Destructive operations — risk: irreversible data loss.
    Destructive,
}

impl ToolRiskCategory {
    /// Human-readable risk description.
    pub fn description(&self) -> &str {
        match self {
            Self::ReadOnly => "Read-only — no risk",
            Self::FileWrite => "File will be modified or overwritten",
            Self::ShellExec => "Arbitrary command execution",
            Self::GitWrite => "Repository state will be changed",
            Self::DatabaseWrite => "Database will be modified",
            Self::NetworkCall => "Data may be sent over the network",
            Self::Destructive => "Irreversible operation — data loss possible",
        }
    }

    /// Risk level: 0 = none, 1 = low, 2 = medium, 3 = high.
    pub fn level(&self) -> u8 {
        match self {
            Self::ReadOnly => 0,
            Self::NetworkCall => 1,
            Self::FileWrite => 2,
            Self::GitWrite => 2,
            Self::DatabaseWrite => 2,
            Self::ShellExec => 3,
            Self::Destructive => 3,
        }
    }

    /// Permission label for display.
    pub fn permission_label(&self) -> &str {
        match self {
            Self::ReadOnly => "Read",
            Self::FileWrite => "Edit",
            Self::ShellExec => "Execute",
            Self::GitWrite => "Git",
            Self::DatabaseWrite => "Database",
            Self::NetworkCall => "Network",
            Self::Destructive => "Delete",
        }
    }
}

/// Classifies tools by risk category based on tool name and parameters.
pub struct ToolRiskClassifier;

impl ToolRiskClassifier {
    /// Classify a tool by name.
    pub fn classify(tool_name: &str) -> ToolRiskCategory {
        match tool_name {
            // Read-only
            "read_file" | "search" | "grep" | "list_files" | "git_log" | "git_status"
            | "git_diff" | "git_blame" | "git_branch" => ToolRiskCategory::ReadOnly,
            // File write
            "apply_patch" | "write_file" | "append_file" | "create_file" | "update_file"
            | "move_file" => ToolRiskCategory::FileWrite,
            // Shell
            "shell" | "execute" => ToolRiskCategory::ShellExec,
            // Git write
            "git_commit" | "git_add" | "git_push" | "git_tag" => ToolRiskCategory::GitWrite,
            // Database
            "db_query" | "db_execute" | "sql" => ToolRiskCategory::DatabaseWrite,
            // Network
            "web_fetch" | "web_search" | "http_request" | "api_call" => {
                ToolRiskCategory::NetworkCall
            }
            // Destructive
            "delete_file" | "rm" | "drop_table" | "truncate" => ToolRiskCategory::Destructive,
            // Default: ReadOnly for unknown tools
            _ => ToolRiskCategory::ReadOnly,
        }
    }

    /// Generate a human-readable safety notice for a tool call.
    pub fn safety_notice(tool_name: &str, params: &ToolParameters) -> String {
        let category = Self::classify(tool_name);
        let path = params
            .get("path")
            .or_else(|| params.get("file_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let command = params.get("command").and_then(|v| v.as_str()).unwrap_or("");

        match category {
            ToolRiskCategory::ReadOnly => String::new(),
            ToolRiskCategory::FileWrite => {
                format!("Editing {path} — risk: file will be modified")
            }
            ToolRiskCategory::ShellExec => {
                format!("Running: {command} — risk: arbitrary command execution")
            }
            ToolRiskCategory::GitWrite => {
                "Git write to repository — risk: repository state change".to_string()
            }
            ToolRiskCategory::DatabaseWrite => {
                "Database modification — risk: data corruption".to_string()
            }
            ToolRiskCategory::NetworkCall => {
                "Network request — risk: data may leave this machine".to_string()
            }
            ToolRiskCategory::Destructive => {
                format!("DELETING {path} — risk: irreversible data loss")
            }
        }
    }
}
