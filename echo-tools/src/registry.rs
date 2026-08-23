//! Canonical standard tool pack.

use echo_core::tools::cell::CommandCellRegistry;
use echo_core::tools::{ToolPack, ToolPackEntry, ToolRegistrar};
use std::sync::Arc;

/// The framework's feature-gated standard tool composition.
///
/// Each entry declares its capabilities at construction time. Read-only
/// surfaces are projections of this pack, not a second hand-maintained tool
/// list, so a mutating tool cannot accidentally enter a read-only Agent.
#[derive(Clone, Default)]
pub struct StandardToolPack {
    cells: Option<Arc<dyn CommandCellRegistry>>,
}

impl StandardToolPack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_command_cells(mut self, cells: Arc<dyn CommandCellRegistry>) -> Self {
        self.cells = Some(cells);
        self
    }
}

impl ToolPack for StandardToolPack {
    fn name(&self) -> &str {
        "standard"
    }

    #[allow(unused_mut)]
    fn tools(&self) -> Vec<ToolPackEntry> {
        let mut tools = Vec::new();

        #[cfg(feature = "artifact")]
        tools.push(ToolPackEntry::read_only(Box::new(
            crate::files::artifact::ReadArtifactTool,
        )));

        #[cfg(feature = "shell")]
        {
            use crate::code::RunCodeTool;
            use crate::shell::ShellTool;
            let shell = self.cells.as_ref().map_or_else(ShellTool::new, |cells| {
                ShellTool::new().with_cell_launcher(Arc::clone(cells))
            });
            tools.push(ToolPackEntry::new(Box::new(shell)));
            tools.push(ToolPackEntry::new(Box::new(RunCodeTool::new())));
        }

        #[cfg(feature = "files")]
        {
            use crate::files::apply_patch::ApplyPatchTool;
            use crate::files::code_search::CodeSearchTool;
            use crate::files::diff::DiffTool;
            use crate::files::files::{ListDirTool, ReadFileTool};
            use crate::files::glob::GlobTool;
            use crate::files::grep::GrepTool;
            use crate::files::repo_map::RepoMapTool;
            tools.extend([
                ToolPackEntry::read_only(Box::new(ReadFileTool::new())),
                ToolPackEntry::read_only(Box::new(ListDirTool::new())),
                ToolPackEntry::read_only(Box::new(GrepTool::new())),
                ToolPackEntry::read_only(Box::new(GlobTool::new())),
                ToolPackEntry::new(Box::new(ApplyPatchTool::new())),
                ToolPackEntry::read_only(Box::new(DiffTool::new())),
                ToolPackEntry::read_only(Box::new(RepoMapTool::new())),
                ToolPackEntry::read_only(Box::new(CodeSearchTool::new())),
            ]);
        }

        #[cfg(feature = "git")]
        {
            use crate::git::{
                GitBlameTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool,
            };
            use crate::worktree_tool::{EnterWorktreeTool, ExitWorktreeTool, ListWorktreesTool};
            tools.extend([
                ToolPackEntry::read_only(Box::new(GitStatusTool)),
                ToolPackEntry::read_only(Box::new(GitDiffTool)),
                ToolPackEntry::read_only(Box::new(GitLogTool)),
                ToolPackEntry::read_only(Box::new(GitBlameTool)),
                ToolPackEntry::new(Box::new(GitBranchTool)),
                ToolPackEntry::new(Box::new(GitCommitTool)),
                ToolPackEntry::new(Box::new(EnterWorktreeTool)),
                ToolPackEntry::new(Box::new(ExitWorktreeTool)),
                ToolPackEntry::read_only(Box::new(ListWorktreesTool)),
            ]);
        }

        #[cfg(feature = "rag")]
        tools.push(ToolPackEntry::read_only(Box::new(
            crate::rag::RagChunkDocumentTool,
        )));

        #[cfg(feature = "chart")]
        tools.push(ToolPackEntry::read_only(Box::new(
            crate::chart::GenerateChartTool,
        )));

        #[cfg(feature = "database")]
        {
            use crate::database::{DescribeTableTool, ListTablesTool, SqlQueryTool};
            tools.extend([
                ToolPackEntry::new(Box::new(SqlQueryTool)),
                ToolPackEntry::read_only(Box::new(ListTablesTool)),
                ToolPackEntry::read_only(Box::new(DescribeTableTool)),
            ]);
        }

        #[cfg(feature = "web")]
        {
            use crate::web::{WebExtractTool, WebFetchTool, WebSearchTool};
            tools.extend([
                ToolPackEntry::read_only(Box::new(WebFetchTool::new())),
                ToolPackEntry::read_only(Box::new(WebExtractTool)),
                ToolPackEntry::read_only(Box::new(WebSearchTool::with_duckduckgo())),
            ]);
        }

        #[cfg(feature = "media")]
        {
            use crate::excel::{
                ExcelInfoTool, ExcelProfileTool, ExcelReadTool, ExcelToCsvTool, ExcelWriteTool,
            };
            use crate::image::ViewImageTool;
            use crate::media::image_fetch::ImageFetchTool;
            use crate::pdf::{PdfExtractTool, PdfInfoTool};
            use crate::text::{TextExportTool, TextProcessTool, TextSearchTool, TextStatsTool};
            use crate::word::{WordInfoTool, WordReadTool, WordStructureTool};
            tools.push(ToolPackEntry::read_only(Box::new(ViewImageTool::new())));
            if let Ok(tool) = ImageFetchTool::new() {
                tools.push(ToolPackEntry::read_only(Box::new(tool)));
            }
            tools.extend([
                ToolPackEntry::read_only(Box::new(PdfExtractTool)),
                ToolPackEntry::read_only(Box::new(PdfInfoTool)),
                ToolPackEntry::read_only(Box::new(ExcelReadTool)),
                ToolPackEntry::read_only(Box::new(ExcelInfoTool)),
                ToolPackEntry::new(Box::new(ExcelToCsvTool)),
                ToolPackEntry::read_only(Box::new(ExcelProfileTool)),
                ToolPackEntry::new(Box::new(ExcelWriteTool)),
                ToolPackEntry::read_only(Box::new(WordReadTool)),
                ToolPackEntry::read_only(Box::new(WordInfoTool)),
                ToolPackEntry::read_only(Box::new(WordStructureTool)),
                ToolPackEntry::read_only(Box::new(TextSearchTool)),
                ToolPackEntry::read_only(Box::new(TextStatsTool)),
                ToolPackEntry::read_only(Box::new(TextProcessTool)),
                ToolPackEntry::new(Box::new(TextExportTool)),
            ]);
            #[cfg(feature = "data")]
            tools.push(ToolPackEntry::new(Box::new(crate::excel::ExcelLoadTool)));
        }

        #[cfg(feature = "data")]
        {
            use crate::data::{
                CorrelateTool, DataAggregateTool, DataBinTool, DataContributionTool,
                DataExportTool, DataFilterTool, DataJoinTool, DataMultiReadTool, DataProfileTool,
                DataRatioTool, DataReadTool, DataStatsTool, DataTopNTool, DataTransformTool,
                PivotTool,
            };
            tools.extend([
                ToolPackEntry::read_only(Box::new(DataReadTool)),
                ToolPackEntry::read_only(Box::new(DataFilterTool)),
                ToolPackEntry::read_only(Box::new(DataAggregateTool)),
                ToolPackEntry::read_only(Box::new(DataStatsTool)),
                ToolPackEntry::read_only(Box::new(DataTransformTool)),
                ToolPackEntry::new(Box::new(DataExportTool)),
                ToolPackEntry::read_only(Box::new(DataProfileTool)),
                ToolPackEntry::read_only(Box::new(DataTopNTool)),
                ToolPackEntry::read_only(Box::new(DataContributionTool)),
                ToolPackEntry::read_only(Box::new(DataBinTool)),
                ToolPackEntry::read_only(Box::new(DataRatioTool)),
                ToolPackEntry::read_only(Box::new(DataMultiReadTool)),
                ToolPackEntry::read_only(Box::new(DataJoinTool)),
                ToolPackEntry::read_only(Box::new(CorrelateTool)),
                ToolPackEntry::read_only(Box::new(PivotTool)),
            ]);
            use crate::data_quality::{
                ConsistencyCheckTool, MissingValueAnalysisTool, OutlierDetectionTool,
            };
            tools.extend([
                ToolPackEntry::read_only(Box::new(MissingValueAnalysisTool)),
                ToolPackEntry::read_only(Box::new(OutlierDetectionTool)),
                ToolPackEntry::read_only(Box::new(ConsistencyCheckTool)),
            ]);
        }

        #[cfg(feature = "statistics")]
        tools.push(ToolPackEntry::read_only(Box::new(
            crate::statistics::ExploratoryStatisticsTool::default(),
        )));

        #[cfg(feature = "research")]
        {
            use crate::research::{
                ArxivSearchTool, BibtexGenerateTool, ClinicalTrialsSearchTool, PdfFetchTool,
                PubMedSearchTool, SemanticScholarSearchTool,
            };
            tools.extend([
                ToolPackEntry::read_only(Box::new(ArxivSearchTool)),
                ToolPackEntry::read_only(Box::new(SemanticScholarSearchTool)),
                ToolPackEntry::read_only(Box::new(PubMedSearchTool)),
                ToolPackEntry::read_only(Box::new(ClinicalTrialsSearchTool)),
                ToolPackEntry::read_only(Box::new(PdfFetchTool)),
                ToolPackEntry::new(Box::new(BibtexGenerateTool)),
            ]);
        }

        tools
    }
}

pub fn register_readonly_tools(registrar: &mut dyn ToolRegistrar) {
    StandardToolPack::new().install_read_only(registrar);
}

pub fn register_all_tools(registrar: &mut dyn ToolRegistrar) {
    StandardToolPack::new().install(registrar);
}

pub fn register_all_tools_with_cells(
    registrar: &mut dyn ToolRegistrar,
    cells: Option<Arc<dyn CommandCellRegistry>>,
) {
    let pack = cells.map_or_else(StandardToolPack::new, |cells| {
        StandardToolPack::new().with_command_cells(cells)
    });
    pack.install(registrar);
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::tools::Tool;

    #[derive(Default)]
    struct Collector(Vec<String>);

    impl ToolRegistrar for Collector {
        fn register(&mut self, tool: Box<dyn Tool>) {
            self.0.push(tool.name().to_string());
        }
    }

    #[test]
    #[cfg(feature = "database")]
    fn read_only_projection_contains_only_read_only_sql() {
        let mut collector = Collector::default();
        register_readonly_tools(&mut collector);
        assert!(collector.0.contains(&"sql_query".to_string()));
        assert!(collector.0.contains(&"list_tables".to_string()));
        assert!(collector.0.contains(&"describe_table".to_string()));
    }

    #[test]
    #[cfg(feature = "shell")]
    fn read_only_projection_excludes_process_execution() {
        let mut collector = Collector::default();
        register_readonly_tools(&mut collector);
        assert!(!collector.0.contains(&"shell".to_string()));
        assert!(!collector.0.contains(&"run_code".to_string()));
    }

    #[test]
    #[cfg(feature = "files")]
    fn standard_pack_uses_only_canonical_patch_mutation() {
        let mut collector = Collector::default();
        register_all_tools(&mut collector);
        assert!(collector.0.contains(&"apply_patch".to_string()));
        for legacy in [
            "edit_file",
            "write_file",
            "append_file",
            "create_file",
            "delete_file",
            "update_file",
            "move_file",
        ] {
            assert!(!collector.0.contains(&legacy.to_string()));
        }
    }
}
