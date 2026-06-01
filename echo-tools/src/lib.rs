//! Echo Tools — Domain tools for the echo-agent framework
//!
//! Feature-gated tool modules that can be registered via
//! [`register_all_tools`] into any [`ToolRegistrar`](echo_core::tools::ToolRegistrar).
//!
//! # Features
//!
//! | Feature   | Tools                                     |
//! |-----------|-------------------------------------------|
//! | `web`     | `web` (WebFetchTool, WebSearchTool, WebExtractTool, providers) |
//! | `chart`   | `chart` (GenerateChartTool)               |
//! | `data`    | `data` (11 data-analysis tools), `data_quality` (3 data-quality tools) |
//! | `statistics`| `statistics` (3 statistical-analysis tools)           |
//! | `database`| `database` (SqlQueryTool, …)              |
//! | `media`   | `excel`, `image`, `pdf`, `word`, `text`, `media` |
//! | `git`     | `git` (6 git CLI tools)                   |
//! | `rag`     | `rag` (RagIndexTool, RagSearchTool, …)    |
//! | `full`    | All of the above                          |

#[cfg(feature = "files")]
#[cfg_attr(docsrs, doc(cfg(feature = "files")))]
pub mod files;
pub mod git_checkpoint;
pub mod security;
#[cfg(feature = "shell")]
#[cfg_attr(docsrs, doc(cfg(feature = "shell")))]
pub mod shell;

#[cfg(feature = "chart")]
#[cfg_attr(docsrs, doc(cfg(feature = "chart")))]
pub mod chart;
#[cfg(feature = "data")]
#[cfg_attr(docsrs, doc(cfg(feature = "data")))]
pub mod data;
#[cfg(feature = "data")]
#[cfg_attr(docsrs, doc(cfg(feature = "data")))]
pub mod data_quality;
#[cfg(feature = "statistics")]
#[cfg_attr(docsrs, doc(cfg(feature = "statistics")))]
pub mod statistics;
#[cfg(feature = "database")]
#[cfg_attr(docsrs, doc(cfg(feature = "database")))]
pub mod database;
#[cfg(feature = "media")]
#[cfg_attr(docsrs, doc(cfg(feature = "media")))]
pub mod excel;
#[cfg(feature = "git")]
#[cfg_attr(docsrs, doc(cfg(feature = "git")))]
pub mod git;
#[cfg(feature = "git")]
#[cfg_attr(docsrs, doc(cfg(feature = "git")))]
pub mod git_worktree;
#[cfg(feature = "git")]
#[cfg_attr(docsrs, doc(cfg(feature = "git")))]
pub mod worktree_tool;
#[cfg(feature = "media")]
#[cfg_attr(docsrs, doc(cfg(feature = "media")))]
pub mod image;
#[cfg(feature = "media")]
#[cfg_attr(docsrs, doc(cfg(feature = "media")))]
pub mod media;
#[cfg(feature = "media")]
#[cfg_attr(docsrs, doc(cfg(feature = "media")))]
pub mod pdf;
#[cfg(feature = "rag")]
#[cfg_attr(docsrs, doc(cfg(feature = "rag")))]
pub mod rag;
#[cfg(feature = "research")]
#[cfg_attr(docsrs, doc(cfg(feature = "research")))]
pub mod research;
#[cfg(feature = "media")]
#[cfg_attr(docsrs, doc(cfg(feature = "media")))]
pub mod text;
#[cfg(feature = "web")]
#[cfg_attr(docsrs, doc(cfg(feature = "web")))]
pub mod web;
#[cfg(feature = "media")]
#[cfg_attr(docsrs, doc(cfg(feature = "media")))]
pub mod word;

mod registry;
pub use registry::register_all_tools;
