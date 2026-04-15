# Web Tools

## What It Is

Web Tools provide internet access capabilities for Agents, including web search and web page content fetching. Agents can proactively search for information, retrieve web page content, and convert it to readable text — no longer limited to offline knowledge.

---

## Problem It Solves

LLMs have knowledge cutoff dates and cannot access real-time information. Web Tools enable Agents to:

- Search for the latest news, tech trends, and real-time data
- Fetch detailed web page content for in-depth analysis
- Verify facts and find citation sources
- Make decisions based on up-to-date information

---

## Architecture

```
WebSearchTool                      ← search tool (implements Tool trait)
    │
SearchProvider trait               ← abstract search engine interface
    ├─ DuckDuckGoProvider          ← free, no API key required (default)
    ├─ BraveSearchProvider         ← paid API, high quality
    └─ TavilyProvider              ← AI-optimized search, designed for agents

WebFetchTool                       ← web page fetcher (implements Tool trait)
    └─ HTML → readable text (html2text)
```

---

## Quick Start

### Basic Usage

```rust
use echo_agent::prelude::*;
use echo_agent::tools::web::{WebSearchTool, WebFetchTool};

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a research assistant")
    .enable_tools()
    .build()?;

// Register Web Tools
agent.add_tool(Box::new(WebSearchTool::with_duckduckgo()));
agent.add_tool(Box::new(WebFetchTool::new()));

let answer = agent.execute("Search for Rust 2024 edition new features").await?;
```

### Auto-select Provider

```rust
// Auto-select by priority: Tavily > Brave > DuckDuckGo
// Reads API keys from env: TAVILY_API_KEY / BRAVE_SEARCH_API_KEY
agent.add_tool(Box::new(WebSearchTool::auto()));
```

---

## WebSearchTool — Web Search

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | Yes | Search query |
| `max_results` | integer | No | Max results to return (default 5, max 10) |

### Output Format

```
Search results (3 results):

1. Rust Programming Language
   https://www.rust-lang.org/
   A language empowering everyone to build reliable and efficient software.

2. Rust Documentation
   https://doc.rust-lang.org/
   The Rust programming language documentation.
```

### Search Providers

#### DuckDuckGo (Free Fallback)

No API key required. Searches by scraping `html.duckduckgo.com`.

```rust
let tool = WebSearchTool::with_duckduckgo();
```

> **Note**: DuckDuckGo depends on HTML page structure. If the layout changes, parsing may break.

#### Brave Search (Paid, High Quality)

Requires API key. 2,000 free queries per month.

```rust
// Option 1: Pass API key directly
let tool = WebSearchTool::with_brave("your-api-key");

// Option 2: Read from BRAVE_SEARCH_API_KEY env var
let tool = WebSearchTool::auto();
```

Get an API key at: https://brave.com/search/api/

#### Tavily (AI-Optimized Search)

Search engine designed specifically for AI agents. Highest snippet quality.

```rust
// Option 1: Pass API key directly
let tool = WebSearchTool::with_tavily("your-api-key");

// Option 2: Read from TAVILY_API_KEY env var
let tool = WebSearchTool::auto();
```

Get an API key at: https://tavily.com/

#### Provider Comparison

| Provider | Cost | Quality | Latency | Notes |
|----------|------|---------|----------|-------|
| DuckDuckGo | Free | Medium | Higher | HTML scraping, no key needed |
| Brave | Free 2k/mo | High | Low | Official API, stable |
| Tavily | Paid (free tier) | Highest | Low | AI-optimized, agent-first design |

---

## WebFetchTool — Web Page Fetching

Fetches content from a given URL and automatically converts HTML to readable text.

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | Yes | Web page URL (must start with http:// or https://) |
| `max_length` | integer | No | Max content length in chars (default 50000) |

### Example

```rust
use echo_agent::tools::web::WebFetchTool;

let tool = WebFetchTool::new()
    .with_max_content_length(10000);  // limit to 10000 chars

agent.add_tool(Box::new(tool));
```

### Features

- Automatic HTML → plain text conversion
- Follows redirects (up to 5)
- Auto-detects content type (HTML / plain text / JSON)
- Automatic truncation for oversized content

---

## Custom SearchProvider

Implement the `SearchProvider` trait to add new search engines:

```rust
use echo_agent::tools::web::providers::{SearchProvider, SearchResult};
use async_trait::async_trait;

struct GoogleProvider {
    api_key: String,
}

#[async_trait]
impl SearchProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    async fn search(&self, query: &str, max_results: usize)
        -> echo_agent::error::Result<Vec<SearchResult>>
    {
        // Call Google Custom Search API ...
        Ok(vec![
            SearchResult {
                title: "Example".into(),
                url: "https://example.com".into(),
                snippet: "An example result".into(),
            },
        ])
    }
}

// Use custom provider
let tool = WebSearchTool::new(Box::new(GoogleProvider { api_key: "xxx".into() }));
```

---

## Feature Flag

Web tools require the `web` feature (included in the `full` feature):

```toml
# Cargo.toml
[dependencies]
echo_agent = { version = "1.3", features = ["web"] }
```

---

## Environment Variables

| Variable | Description |
|----------|-------------|
| `BRAVE_SEARCH_API_KEY` | Brave Search API key |
| `TAVILY_API_KEY` | Tavily API key |

---

See: `examples/demo41_web_tools.rs`
