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
//! | `data`    | `data` (11 data-analysis tools)           |
//! | `database`| `database` (SqlQueryTool, …)              |
//! | `media`   | `excel`, `image`, `pdf`, `word`, `text`, `media` |
//! | `git`     | `git` (6 git CLI tools)                   |
//! | `rag`     | `rag` (RagIndexTool, RagSearchTool, …)    |
//! | `full`    | All of the above                          |

#[cfg(feature = "files")]
pub mod files;
pub mod security;
#[cfg(feature = "shell")]
pub mod shell;

#[cfg(feature = "chart")]
pub mod chart;
#[cfg(feature = "data")]
pub mod data;
#[cfg(feature = "database")]
pub mod database;
#[cfg(feature = "media")]
pub mod excel;
#[cfg(feature = "git")]
pub mod git;
#[cfg(feature = "media")]
pub mod image;
#[cfg(feature = "media")]
pub mod media;
#[cfg(feature = "media")]
pub mod pdf;
#[cfg(feature = "rag")]
pub mod rag;
#[cfg(feature = "research")]
pub mod research;
#[cfg(feature = "media")]
pub mod text;
#[cfg(feature = "web")]
pub mod web;
#[cfg(feature = "media")]
pub mod word;

mod registry;
pub use registry::register_all_tools;
