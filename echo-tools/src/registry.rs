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
    #[cfg(feature = "files")]
    file_system: Option<Arc<dyn echo_core::tools::filesystem::FileSystem>>,
    #[cfg(feature = "files")]
    content_search_provider: Option<Arc<dyn echo_core::tools::search::ContentSearchProvider>>,
}

impl StandardToolPack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_command_cells(mut self, cells: Arc<dyn CommandCellRegistry>) -> Self {
        self.cells = Some(cells);
        self
    }

    /// Bind the existing file tools to one immutable target. Invocation cwd
    /// still selects the actual checkout within that target.
    #[cfg(feature = "files")]
    pub fn with_file_system(
        mut self,
        file_system: Arc<dyn echo_core::tools::filesystem::FileSystem>,
    ) -> Self {
        self.file_system = Some(file_system);
        self
    }

    /// 可选远端内容搜索接缝，只作用于 `grep`。
    ///
    /// `None`（默认）时 grep 保持本机遍历；提供者说“不支持”的单次请求也
    /// 落回同一路径，因此未注入者与既有部署行为完全一致。
    #[cfg(feature = "files")]
    pub fn with_content_search_provider(
        mut self,
        provider: Option<Arc<dyn echo_core::tools::search::ContentSearchProvider>>,
    ) -> Self {
        self.content_search_provider = provider;
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
            #[allow(unused_mut)]
            let mut run_code = RunCodeTool::new();
            #[cfg(feature = "files")]
            if let Some(file_system) = &self.file_system {
                run_code = run_code.with_file_system(Arc::clone(file_system));
            }
            tools.push(ToolPackEntry::new(Box::new(run_code)));
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
            let file_system = self
                .file_system
                .clone()
                .unwrap_or_else(|| Arc::new(crate::files::LocalFileSystem));
            tools.extend([
                ToolPackEntry::read_only(Box::new(
                    ReadFileTool::new().with_file_system(Arc::clone(&file_system)),
                )),
                ToolPackEntry::read_only(Box::new(
                    ListDirTool::new().with_file_system(Arc::clone(&file_system)),
                )),
                ToolPackEntry::read_only(Box::new(
                    GrepTool::new()
                        .with_file_system(Arc::clone(&file_system))
                        .with_search_provider(self.content_search_provider.clone()),
                )),
                ToolPackEntry::read_only(Box::new(
                    GlobTool::new().with_file_system(Arc::clone(&file_system)),
                )),
                ToolPackEntry::new(Box::new(
                    ApplyPatchTool::new().with_file_system(Arc::clone(&file_system)),
                )),
                ToolPackEntry::read_only(Box::new(
                    DiffTool::new().with_file_system(Arc::clone(&file_system)),
                )),
                ToolPackEntry::read_only(Box::new(
                    RepoMapTool::new().with_file_system(Arc::clone(&file_system)),
                )),
                ToolPackEntry::read_only(Box::new(
                    CodeSearchTool::new().with_file_system(file_system),
                )),
            ]);
        }

        #[cfg(feature = "git")]
        {
            use crate::git::{
                GitBlameTool, GitBranchTool, GitCommitTool, GitDiffTool, GitLogTool, GitStatusTool,
            };
            use crate::worktree_tool::{EnterWorktreeTool, ExitWorktreeTool, ListWorktreesTool};
            #[cfg(feature = "files")]
            let git_file_system = self.file_system.clone();
            #[cfg(not(feature = "files"))]
            let git_file_system = None;
            tools.extend([
                ToolPackEntry::read_only(crate::git::bind_file_system(
                    GitStatusTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::read_only(crate::git::bind_file_system(
                    GitDiffTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::read_only(crate::git::bind_file_system(
                    GitLogTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::read_only(crate::git::bind_file_system(
                    GitBlameTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::new(crate::git::bind_file_system(
                    GitBranchTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::new(crate::git::bind_file_system(
                    GitCommitTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::new(crate::git::bind_file_system(
                    EnterWorktreeTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::new(crate::git::bind_file_system(
                    ExitWorktreeTool,
                    git_file_system.clone(),
                )),
                ToolPackEntry::read_only(crate::git::bind_file_system(
                    ListWorktreesTool,
                    git_file_system.clone(),
                )),
            ]);
        }

        #[cfg(feature = "rag")]
        tools.push(ToolPackEntry::read_only(Box::new(
            crate::rag::RagChunkDocumentTool,
        )));

        #[cfg(feature = "chart")]
        tools.extend([ToolPackEntry::read_only(Box::new(
            crate::chart::GenerateChartTool,
        ))]);

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
    #[cfg(any(feature = "database", feature = "files", feature = "shell"))]
    use super::*;
    #[cfg(any(feature = "database", feature = "files", feature = "shell"))]
    use echo_core::tools::Tool;

    #[cfg(any(feature = "database", feature = "files", feature = "shell"))]
    #[derive(Default)]
    struct Collector(Vec<String>);

    #[cfg(any(feature = "database", feature = "files", feature = "shell"))]
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

    // ── content search seam: default pack stays local, provider is opt-in ──

    #[cfg(feature = "files")]
    use echo_core::tools::search::{
        ContentSearchOutcome, ContentSearchProvider, ContentSearchRequest,
    };
    #[cfg(feature = "files")]
    use echo_core::tools::{ToolContext, ToolParameters};

    #[cfg(feature = "files")]
    #[derive(Default)]
    struct ToolCollector(Vec<Box<dyn Tool>>);

    #[cfg(feature = "files")]
    impl ToolRegistrar for ToolCollector {
        fn register(&mut self, tool: Box<dyn Tool>) {
            self.0.push(tool);
        }
    }

    #[cfg(feature = "files")]
    fn grep_from(pack: &StandardToolPack) -> std::result::Result<Box<dyn Tool>, String> {
        let mut collector = ToolCollector::default();
        pack.install(&mut collector);
        collector
            .0
            .into_iter()
            .find(|tool| tool.name() == "grep")
            .ok_or_else(|| "standard pack did not install grep".to_string())
    }

    #[cfg(feature = "files")]
    fn grep_params(pattern: &str, path: &str) -> ToolParameters {
        ToolParameters::from([
            ("pattern".to_string(), serde_json::json!(pattern)),
            ("path".to_string(), serde_json::json!(path)),
        ])
    }

    /// 只回传一条远端记录；`root` 在本机不存在，本机遍历必然失败。
    #[cfg(feature = "files")]
    struct RemoteOnlyProvider;

    #[cfg(feature = "files")]
    impl ContentSearchProvider for RemoteOnlyProvider {
        fn search(
            &self,
            request: &ContentSearchRequest<'_>,
        ) -> echo_core::error::Result<Option<ContentSearchOutcome>> {
            Ok(Some(ContentSearchOutcome {
                records: vec![format!("{}:1:  remote-pack-hit", request.root.display())],
                truncated: false,
            }))
        }
    }

    #[cfg(feature = "files")]
    #[tokio::test]
    async fn standard_pack_default_grep_keeps_local_behavior() -> std::result::Result<(), String> {
        // 默认工具包（未设置提供者）与直接构造 GrepTool 的输出必须逐字相同，
        // 这是“接入接缝后默认路径行为不变”的回归点。
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let working = std::fs::canonicalize(dir.path()).map_err(|error| error.to_string())?;
        std::fs::write(working.join("notes.txt"), "TODO fix this\n")
            .map_err(|error| error.to_string())?;
        let ctx = ToolContext {
            working_dir: Some(working.clone()),
            ..Default::default()
        };

        let from_pack = grep_from(&StandardToolPack::new())?
            .execute_with_context(grep_params("TODO", "notes.txt"), &ctx)
            .await
            .map_err(|error| error.to_string())?;
        let direct = crate::files::grep::GrepTool::new()
            .execute_with_context(grep_params("TODO", "notes.txt"), &ctx)
            .await
            .map_err(|error| error.to_string())?;

        assert!(from_pack.success, "{}", from_pack.error.unwrap_or_default());
        assert_eq!(from_pack.output, direct.output);
        assert_eq!(from_pack.truncated, direct.truncated);
        assert!(from_pack.output.contains("fix this"));
        Ok(())
    }

    #[cfg(feature = "files")]
    #[tokio::test]
    async fn standard_pack_forwards_content_search_provider_to_grep()
    -> std::result::Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let working = std::fs::canonicalize(dir.path()).map_err(|error| error.to_string())?;
        let ctx = ToolContext {
            working_dir: Some(working),
            ..Default::default()
        };
        let pack = StandardToolPack::new()
            .with_content_search_provider(Some(Arc::new(RemoteOnlyProvider)));
        let result = grep_from(&pack)?
            .execute_with_context(grep_params("needle", "remote-project"), &ctx)
            .await
            .map_err(|error| error.to_string())?;

        assert!(result.success, "{}", result.error.unwrap_or_default());
        assert!(result.output.contains("remote-pack-hit"));
        Ok(())
    }
}
