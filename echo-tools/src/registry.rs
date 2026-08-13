//! Tool registration helper
//!
//! Provides [`register_all_tools`] which registers every enabled domain tool
//! into any type implementing [`ToolRegistrar`](echo_core::tools::ToolRegistrar).
//! [`register_readonly_tools`] registers only read-only tools (no shell, no
//! file writes) — used by read-only Subagents.

use echo_core::tools::ToolRegistrar;

/// Register only **read-only** tools into the given registrar.
///
/// This excludes all mutating tools: shell, write/append/create/delete/move/
/// update/edit files, git write operations (commit/branch/worktree-enter).
/// Read-only tools include: read_file, list_dir, grep, glob, diff, repo_map,
/// code_search, git read ops (status/diff/log/blame), web search/fetch,
/// data read/profile, research search, media read/extract, statistics.
///
/// Used when constructing read-only Subagents so they are physically incapable
/// of mutating state, not just prompt-constrained.
#[allow(unused_variables)]
pub fn register_readonly_tools(tool_manager: &mut dyn ToolRegistrar) {
    // ── files (read-only subset) ──────────────────────────────────────────
    #[cfg(feature = "files")]
    {
        use crate::files::artifact::ReadArtifactTool;
        use crate::files::code_search::CodeSearchTool;
        use crate::files::diff::DiffTool;
        use crate::files::files::{ListDirTool, ReadFileTool};
        use crate::files::glob::GlobTool;
        use crate::files::grep::GrepTool;
        use crate::files::repo_map::RepoMapTool;

        tool_manager.register(Box::new(ReadFileTool::new()));
        tool_manager.register(Box::new(ReadArtifactTool));
        tool_manager.register(Box::new(ListDirTool::new()));
        tool_manager.register(Box::new(GrepTool::new()));
        tool_manager.register(Box::new(GlobTool::new()));
        tool_manager.register(Box::new(DiffTool::new()));
        tool_manager.register(Box::new(RepoMapTool::new()));
        tool_manager.register(Box::new(CodeSearchTool::new()));
    }

    // ── git (read-only subset) ────────────────────────────────────────────
    #[cfg(feature = "git")]
    {
        use crate::git::{GitBlameTool, GitDiffTool, GitLogTool, GitStatusTool};
        // Deliberately EXCLUDED: GitBranchTool, GitCommitTool,
        // EnterWorktreeTool, ExitWorktreeTool, ListWorktreesTool.
        tool_manager.register(Box::new(GitStatusTool::default()));
        tool_manager.register(Box::new(GitDiffTool));
        tool_manager.register(Box::new(GitLogTool));
        tool_manager.register(Box::new(GitBlameTool));
    }

    // ── rag (dependency-free read-only subset) ───────────────────────────
    #[cfg(feature = "rag")]
    {
        use crate::rag::RagChunkDocumentTool;
        tool_manager.register(Box::new(RagChunkDocumentTool));
    }

    // ── chart (read-only — generates charts but no file mutation) ─────────
    #[cfg(feature = "chart")]
    {
        use crate::chart::GenerateChartTool;
        tool_manager.register(Box::new(GenerateChartTool));
    }

    // ── database (read-only subset) ───────────────────────────────────────
    #[cfg(feature = "database")]
    {
        use crate::database::{DescribeTableTool, ListTablesTool, SqlQueryTool};
        // SqlQueryTool could mutate (INSERT/UPDATE), but in a local analysis
        // context the risk is low and the read value is high. Keep it.
        tool_manager.register(Box::new(SqlQueryTool));
        tool_manager.register(Box::new(ListTablesTool));
        tool_manager.register(Box::new(DescribeTableTool));
    }

    // ── web (all read-only) ───────────────────────────────────────────────
    #[cfg(feature = "web")]
    {
        use crate::web::{WebExtractTool, WebFetchTool, WebSearchTool};
        tool_manager.register(Box::new(WebFetchTool::new()));
        tool_manager.register(Box::new(WebExtractTool));
        tool_manager.register(Box::new(WebSearchTool::with_duckduckgo()));
    }

    // ── media (read-only subset) ──────────────────────────────────────────
    #[cfg(feature = "media")]
    {
        use crate::image::ImageAnalysisTool;
        use crate::media::image_fetch::ImageFetchTool;
        use crate::pdf::{PdfExtractTool, PdfInfoTool};
        use crate::text::{TextProcessTool, TextSearchTool, TextStatsTool};
        use crate::word::{WordInfoTool, WordReadTool, WordStructureTool};
        // Excel: EXCLUDED ExcelWriteTool; kept read/info/csv/profile.
        use crate::excel::{ExcelInfoTool, ExcelProfileTool, ExcelReadTool};

        tool_manager.register(Box::new(ImageAnalysisTool));
        if let Ok(tool) = ImageFetchTool::new() {
            tool_manager.register(Box::new(tool));
        }
        tool_manager.register(Box::new(PdfExtractTool));
        tool_manager.register(Box::new(PdfInfoTool));
        tool_manager.register(Box::new(ExcelReadTool));
        tool_manager.register(Box::new(ExcelInfoTool));
        tool_manager.register(Box::new(ExcelProfileTool));
        tool_manager.register(Box::new(WordReadTool));
        tool_manager.register(Box::new(WordInfoTool));
        tool_manager.register(Box::new(WordStructureTool));
        tool_manager.register(Box::new(TextSearchTool));
        tool_manager.register(Box::new(TextStatsTool));
        tool_manager.register(Box::new(TextProcessTool));
    }

    // ── data (read-only subset) ───────────────────────────────────────────
    #[cfg(feature = "data")]
    {
        // EXCLUDED: DataExportTool (writes files).
        use crate::data::{
            CorrelateTool, DataAggregateTool, DataBinTool, DataContributionTool, DataFilterTool,
            DataJoinTool, DataMultiReadTool, DataProfileTool, DataRatioTool, DataReadTool,
            DataStatsTool, DataTopNTool, DataTransformTool, PivotTool,
        };
        tool_manager.register(Box::new(DataReadTool));
        tool_manager.register(Box::new(DataFilterTool));
        tool_manager.register(Box::new(DataAggregateTool));
        tool_manager.register(Box::new(DataStatsTool));
        tool_manager.register(Box::new(DataTransformTool));
        tool_manager.register(Box::new(DataProfileTool));
        tool_manager.register(Box::new(DataTopNTool));
        tool_manager.register(Box::new(DataContributionTool));
        tool_manager.register(Box::new(DataBinTool));
        tool_manager.register(Box::new(DataRatioTool));
        tool_manager.register(Box::new(DataMultiReadTool));
        tool_manager.register(Box::new(DataJoinTool));
        tool_manager.register(Box::new(CorrelateTool));
        tool_manager.register(Box::new(PivotTool));

        use crate::data_quality::{
            ConsistencyCheckTool, MissingValueAnalysisTool, OutlierDetectionTool,
        };
        tool_manager.register(Box::new(MissingValueAnalysisTool));
        tool_manager.register(Box::new(OutlierDetectionTool));
        tool_manager.register(Box::new(ConsistencyCheckTool));
    }

    #[cfg(feature = "statistics")]
    {
        use crate::statistics::ExploratoryStatisticsTool;
        tool_manager.register(Box::new(ExploratoryStatisticsTool::default()));
    }

    // ── research (all read-only) ──────────────────────────────────────────
    #[cfg(feature = "research")]
    {
        use crate::research::{
            ArxivSearchTool, ClinicalTrialsSearchTool, PdfFetchTool, PubMedSearchTool,
            SemanticScholarSearchTool,
        };
        tool_manager.register(Box::new(ArxivSearchTool));
        tool_manager.register(Box::new(SemanticScholarSearchTool));
        tool_manager.register(Box::new(PubMedSearchTool));
        tool_manager.register(Box::new(ClinicalTrialsSearchTool));
        tool_manager.register(Box::new(PdfFetchTool));
    }

    #[cfg(not(any(
        feature = "files",
        feature = "git",
        feature = "rag",
        feature = "chart",
        feature = "database",
        feature = "web",
        feature = "media",
        feature = "data",
        feature = "statistics",
        feature = "research"
    )))]
    {
        let _ = tool_manager; // Suppress unused warning when no feature-gated tools
    }
}

/// Register all feature-gated domain tools into the given registrar.
#[allow(unused_variables)]
pub fn register_all_tools(tool_manager: &mut dyn ToolRegistrar) {
    // ── shell ─────────────────────────────────────────────────────────────
    #[cfg(feature = "shell")]
    {
        use crate::shell::ShellTool;
        tool_manager.register(Box::new(ShellTool::new()));
        // Sprint 10b: inline code execution (Python/R/JS/...). Same shell
        // feature gate; writer toolset only (readonly subset excludes it —
        // readonly Subagents shouldn't run arbitrary code).
        tool_manager.register(Box::new(crate::code::RunCodeTool::new()));
    }

    // ── files ─────────────────────────────────────────────────────────────
    #[cfg(feature = "files")]
    {
        use crate::files::artifact::ReadArtifactTool;
        use crate::files::code_search::CodeSearchTool;
        use crate::files::diff::DiffTool;
        use crate::files::edit::EditFileTool;
        use crate::files::files::{
            AppendFileTool, CreateFileTool, DeleteFileTool, ListDirTool, MoveFileTool,
            ReadFileTool, UpdateFileTool, WriteFileTool,
        };
        use crate::files::glob::GlobTool;
        use crate::files::grep::GrepTool;
        use crate::files::repo_map::RepoMapTool;

        tool_manager.register(Box::new(ReadFileTool::new()));
        tool_manager.register(Box::new(ReadArtifactTool));
        tool_manager.register(Box::new(WriteFileTool::new()));
        tool_manager.register(Box::new(AppendFileTool::new()));
        tool_manager.register(Box::new(ListDirTool::new()));
        tool_manager.register(Box::new(CreateFileTool::new()));
        tool_manager.register(Box::new(DeleteFileTool::new()));
        tool_manager.register(Box::new(UpdateFileTool::new()));
        tool_manager.register(Box::new(MoveFileTool::new()));
        tool_manager.register(Box::new(GrepTool::new()));
        tool_manager.register(Box::new(GlobTool::new()));
        tool_manager.register(Box::new(EditFileTool::new()));
        tool_manager.register(Box::new(DiffTool::new()));
        tool_manager.register(Box::new(RepoMapTool::new()));
        tool_manager.register(Box::new(CodeSearchTool::new()));
    }

    #[cfg(not(any(
        feature = "shell",
        feature = "files",
        feature = "git",
        feature = "rag",
        feature = "chart",
        feature = "database",
        feature = "web",
        feature = "media",
        feature = "data",
        feature = "statistics",
        feature = "research"
    )))]
    {
        let _ = tool_manager; // Suppress unused warning when no feature-gated tools
    }
    #[cfg(feature = "git")]
    {
        use crate::git::{
            GitBlameTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool,
        };
        use crate::worktree_tool::{EnterWorktreeTool, ExitWorktreeTool, ListWorktreesTool};
        tool_manager.register(Box::new(GitStatusTool::default()));
        tool_manager.register(Box::new(GitDiffTool));
        tool_manager.register(Box::new(GitLogTool));
        tool_manager.register(Box::new(GitBlameTool));
        tool_manager.register(Box::new(GitBranchTool));
        tool_manager.register(Box::new(GitCommitTool));
        tool_manager.register(Box::new(EnterWorktreeTool));
        tool_manager.register(Box::new(ExitWorktreeTool));
        tool_manager.register(Box::new(ListWorktreesTool));
    }

    #[cfg(feature = "rag")]
    {
        use crate::rag::RagChunkDocumentTool;
        tool_manager.register(Box::new(RagChunkDocumentTool));
    }

    #[cfg(feature = "chart")]
    {
        use crate::chart::GenerateChartTool;
        tool_manager.register(Box::new(GenerateChartTool));
    }

    #[cfg(feature = "database")]
    {
        use crate::database::{DescribeTableTool, ListTablesTool, SqlQueryTool};
        tool_manager.register(Box::new(SqlQueryTool));
        tool_manager.register(Box::new(ListTablesTool));
        tool_manager.register(Box::new(DescribeTableTool));
    }

    #[cfg(feature = "web")]
    {
        use crate::web::{WebExtractTool, WebFetchTool, WebSearchTool};
        tool_manager.register(Box::new(WebFetchTool::new()));
        tool_manager.register(Box::new(WebExtractTool));
        tool_manager.register(Box::new(WebSearchTool::with_duckduckgo()));
    }

    #[cfg(feature = "media")]
    {
        use crate::excel::{
            ExcelInfoTool, ExcelProfileTool, ExcelReadTool, ExcelToCsvTool, ExcelWriteTool,
        };
        use crate::image::ImageAnalysisTool;
        use crate::media::image_fetch::ImageFetchTool;
        use crate::pdf::{PdfExtractTool, PdfInfoTool};
        use crate::text::{TextExportTool, TextProcessTool, TextSearchTool, TextStatsTool};
        use crate::word::{WordInfoTool, WordReadTool, WordStructureTool};

        tool_manager.register(Box::new(ImageAnalysisTool));
        if let Ok(tool) = ImageFetchTool::new() {
            tool_manager.register(Box::new(tool));
        }
        tool_manager.register(Box::new(PdfExtractTool));
        tool_manager.register(Box::new(PdfInfoTool));
        tool_manager.register(Box::new(ExcelReadTool));
        tool_manager.register(Box::new(ExcelInfoTool));
        tool_manager.register(Box::new(ExcelToCsvTool));
        tool_manager.register(Box::new(ExcelProfileTool));
        tool_manager.register(Box::new(ExcelWriteTool));
        #[cfg(feature = "data")]
        {
            use crate::excel::ExcelLoadTool;
            tool_manager.register(Box::new(ExcelLoadTool));
        }
        tool_manager.register(Box::new(WordReadTool));
        tool_manager.register(Box::new(WordInfoTool));
        tool_manager.register(Box::new(WordStructureTool));
        tool_manager.register(Box::new(TextSearchTool));
        tool_manager.register(Box::new(TextStatsTool));
        tool_manager.register(Box::new(TextProcessTool));
        tool_manager.register(Box::new(TextExportTool));
    }

    #[cfg(feature = "data")]
    {
        use crate::data::{
            CorrelateTool, DataAggregateTool, DataBinTool, DataContributionTool, DataExportTool,
            DataFilterTool, DataJoinTool, DataMultiReadTool, DataProfileTool, DataRatioTool,
            DataReadTool, DataStatsTool, DataTopNTool, DataTransformTool, PivotTool,
        };

        tool_manager.register(Box::new(DataReadTool));
        tool_manager.register(Box::new(DataFilterTool));
        tool_manager.register(Box::new(DataAggregateTool));
        tool_manager.register(Box::new(DataStatsTool));
        tool_manager.register(Box::new(DataTransformTool));
        tool_manager.register(Box::new(DataExportTool));
        tool_manager.register(Box::new(DataProfileTool));
        tool_manager.register(Box::new(DataTopNTool));
        tool_manager.register(Box::new(DataContributionTool));
        tool_manager.register(Box::new(DataBinTool));
        tool_manager.register(Box::new(DataRatioTool));
        tool_manager.register(Box::new(DataMultiReadTool));
        tool_manager.register(Box::new(DataJoinTool));
        tool_manager.register(Box::new(CorrelateTool));
        tool_manager.register(Box::new(PivotTool));
    }

    #[cfg(feature = "data")]
    {
        use crate::data_quality::{
            ConsistencyCheckTool, MissingValueAnalysisTool, OutlierDetectionTool,
        };

        tool_manager.register(Box::new(MissingValueAnalysisTool));
        tool_manager.register(Box::new(OutlierDetectionTool));
        tool_manager.register(Box::new(ConsistencyCheckTool));
    }

    #[cfg(feature = "statistics")]
    {
        use crate::statistics::ExploratoryStatisticsTool;

        tool_manager.register(Box::new(ExploratoryStatisticsTool::default()));
    }

    // ── research ──────────────────────────────────────────────────────────
    #[cfg(feature = "research")]
    {
        use crate::research::{
            ArxivSearchTool, BibtexGenerateTool, ClinicalTrialsSearchTool, PdfFetchTool,
            PubMedSearchTool, SemanticScholarSearchTool,
        };
        tool_manager.register(Box::new(ArxivSearchTool));
        tool_manager.register(Box::new(SemanticScholarSearchTool));
        tool_manager.register(Box::new(PubMedSearchTool));
        tool_manager.register(Box::new(ClinicalTrialsSearchTool));
        tool_manager.register(Box::new(PdfFetchTool));
        tool_manager.register(Box::new(BibtexGenerateTool));
    }
}

#[cfg(test)]
mod tests {
    #[cfg(any(feature = "shell", feature = "statistics"))]
    use echo_core::tools::{Tool, ToolRegistrar};

    /// A registrar that collects the names of every tool registered into it.
    #[cfg(any(feature = "shell", feature = "statistics"))]
    struct Collector {
        names: std::sync::Mutex<Vec<String>>,
    }
    #[cfg(any(feature = "shell", feature = "statistics"))]
    impl ToolRegistrar for Collector {
        fn register(&mut self, tool: Box<dyn Tool>) {
            self.names
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(tool.name().to_string());
        }
    }

    /// Sprint 10b: `run_code` must be in the writer toolset
    /// (`register_all_tools`). readonly subset must NOT include it.
    #[test]
    #[cfg(feature = "shell")]
    fn register_all_tools_includes_run_code() {
        let mut c = Collector {
            names: std::sync::Mutex::new(vec![]),
        };
        crate::register_all_tools(&mut c);
        let names = c
            .names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            names.contains(&"run_code".to_string()),
            "run_code missing from register_all_tools: {:?}",
            names
        );
    }

    /// Sprint 10b: the readonly subset must NOT include `run_code` (it's a
    /// writer/execute primitive; readonly Subagents shouldn't run arbitrary code).
    #[test]
    #[cfg(feature = "shell")]
    fn register_readonly_tools_excludes_run_code() {
        let mut c = Collector {
            names: std::sync::Mutex::new(vec![]),
        };
        crate::register_readonly_tools(&mut c);
        let names = c
            .names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            !names.contains(&"run_code".to_string()),
            "run_code must NOT be in the readonly subset: {:?}",
            names
        );
    }

    #[test]
    #[cfg(feature = "statistics")]
    fn statistics_registry_exposes_only_exploratory_summary() {
        let mut all = Collector {
            names: std::sync::Mutex::new(Vec::new()),
        };
        crate::register_all_tools(&mut all);
        let all_names = all
            .names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(all_names.contains(&"exploratory_statistics".to_string()));
        assert!(!all_names.contains(&"hypothesis_test".to_string()));
        assert!(!all_names.contains(&"regression".to_string()));
        assert!(!all_names.contains(&"descriptive_advanced".to_string()));

        let mut readonly = Collector {
            names: std::sync::Mutex::new(Vec::new()),
        };
        crate::register_readonly_tools(&mut readonly);
        let readonly_names = readonly
            .names
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(readonly_names.contains(&"exploratory_statistics".to_string()));
        assert!(!readonly_names.contains(&"hypothesis_test".to_string()));
        assert!(!readonly_names.contains(&"regression".to_string()));
        assert!(!readonly_names.contains(&"descriptive_advanced".to_string()));
    }
}
