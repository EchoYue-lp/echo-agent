//! Tool registration helper
//!
//! Provides [`register_all_tools`] which registers every enabled domain tool
//! into any type implementing [`ToolRegistrar`](echo_core::tools::ToolRegistrar).

use echo_core::tools::ToolRegistrar;

/// Register all feature-gated domain tools into the given registrar.
#[allow(unused_variables)]
pub fn register_all_tools(tool_manager: &mut dyn ToolRegistrar) {
    // ── shell ─────────────────────────────────────────────────────────────
    #[cfg(feature = "shell")]
    {
        use crate::shell::ShellTool;
        tool_manager.register(Box::new(ShellTool::new()));
    }

    // ── files ─────────────────────────────────────────────────────────────
    #[cfg(feature = "files")]
    {
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
        feature = "research",
        feature = "statistics"
    )))]
    {
        let _ = tool_manager; // Suppress unused warning when no feature-gated tools
    }
    #[cfg(feature = "git")]
    {
        use crate::git::{
            GitBlameTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool,
        };
        tool_manager.register(Box::new(GitStatusTool::default()));
        tool_manager.register(Box::new(GitDiffTool));
        tool_manager.register(Box::new(GitLogTool));
        tool_manager.register(Box::new(GitBlameTool));
        tool_manager.register(Box::new(GitBranchTool));
        tool_manager.register(Box::new(GitCommitTool));
    }

    #[cfg(feature = "rag")]
    {
        use crate::rag::{RagChunkDocumentTool, RagIndexTool, RagSearchTool};
        tool_manager.register(Box::new(RagIndexTool));
        tool_manager.register(Box::new(RagSearchTool));
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
        use crate::media::web_fetch_enhanced::WebFetchToolEnhanced;
        use crate::pdf::{PdfExtractTool, PdfInfoTool};
        use crate::text::{
            TextExportTool, TextProcessTool, TextReadTool, TextSearchTool, TextStatsTool,
        };
        use crate::word::{WordInfoTool, WordReadTool, WordStructureTool};

        tool_manager.register(Box::new(ImageAnalysisTool));
        if let Ok(tool) = ImageFetchTool::new() {
            tool_manager.register(Box::new(tool));
        }
        tool_manager.register(Box::new(WebFetchToolEnhanced::new()));
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
        tool_manager.register(Box::new(TextReadTool));
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
        use crate::statistics::{
            DescriptiveAdvancedTool, HypothesisTestTool, RegressionTool,
        };

        tool_manager.register(Box::new(HypothesisTestTool::default()));
        tool_manager.register(Box::new(RegressionTool::default()));
        tool_manager.register(Box::new(DescriptiveAdvancedTool::default()));
    }

    // ── research ──────────────────────────────────────────────────────────
    #[cfg(feature = "research")]
    {
        use crate::research::{
            ArxivSearchTool, BibtexGenerateTool, PdfFetchTool, ResearchRecallTool,
            ResearchRememberTool, SemanticScholarSearchTool,
        };
        tool_manager.register(Box::new(ArxivSearchTool));
        tool_manager.register(Box::new(SemanticScholarSearchTool));
        tool_manager.register(Box::new(PdfFetchTool));
        tool_manager.register(Box::new(BibtexGenerateTool));
        tool_manager.register(Box::new(ResearchRememberTool));
        tool_manager.register(Box::new(ResearchRecallTool));
    }
}
