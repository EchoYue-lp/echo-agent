//! Web tools module
//!
//! Provides web search and page fetching capabilities:
//!
//! - [`WebSearchTool`][]: Search for information on the internet
//! - [`WebFetchTool`][]: Fetch web page content and convert to readable text
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use echo_agent::tools::web::{WebSearchTool, WebFetchTool};
//!
//! let search_tool = WebSearchTool::with_duckduckgo();
//! let fetch_tool = WebFetchTool::new();
//! ```

pub mod fetch;
pub mod providers;
pub mod search;

pub use fetch::WebFetchTool;
pub use search::WebSearchTool;
