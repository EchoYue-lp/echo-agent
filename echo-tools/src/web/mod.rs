//! Web tools module
//!
//! Provides web search, page fetching, and content extraction capabilities:
//!
//! - [`WebSearchTool`][]: Search for information on the internet
//! - [`WebFetchTool`][]: Fetch web page content and convert to readable text
//! - [`WebExtractTool`][]: Extract structured content from HTML

pub mod extract;
pub mod fetch;
pub mod providers;
pub mod search;

pub use extract::WebExtractTool;
pub use fetch::WebFetchTool;
pub use search::WebSearchTool;
