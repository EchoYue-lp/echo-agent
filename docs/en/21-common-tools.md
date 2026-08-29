# Common Tools

## What This Page Covers

This page is a practical guide to the most commonly used tool categories in `echo-agent`:

- `WebSearchTool` for real-time discovery
- `WebFetchTool` for reading page content
- browser automation tools exposed through Playwright MCP
- built-in data processing tools for CSV / JSON / Parquet / Excel / Word / text files

Use this as a "which tool should I reach for?" reference.

---

## Quick Selection Guide

| Goal | Recommended Tool | Best For |
|------|------------------|----------|
| Find the latest information | `WebSearchTool` | News, docs, current events, external references |
| Read a specific page | `WebFetchTool` | Blog posts, docs pages, article bodies |
| Interact with a website | Playwright MCP tools | Clicking, typing, navigation, screenshots |
| Analyze local data files | Data tools | CSV / JSON / Parquet statistics and transformations |
| Read office documents | `read_excel`, `excel_info`, `read_word`, `word_info` | Spreadsheet / Word previews |

---

## 1. Web Search

`WebSearchTool` is the best starting point when the Agent does not yet know which URL matters.

Typical use cases:

- "Search Rust 2024 release notes"
- "Find the latest pricing page for product X"
- "Look up recent AI benchmark updates"

Quick start:

```rust
use echo_agent::prelude::*;
use echo_agent::tools::web::WebSearchTool;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a research assistant.")
    .enable_tools()
    .build()?;

agent.add_tool(Box::new(WebSearchTool::auto()));
```

Provider priority in `auto()`:

- `TAVILY_API_KEY` -> Tavily
- `BRAVE_SEARCH_API_KEY` -> Brave
- otherwise DuckDuckGo

See: `echo-agent-learning/examples/demo41_web_tools.rs`

---

## 2. Web Fetch

`WebFetchTool` is for cases where you already have a URL and want readable content.

Typical use cases:

- fetch a product documentation page
- fetch a changelog entry
- fetch an official blog post for summarization

Quick start:

```rust
use echo_agent::tools::web::WebFetchTool;

agent.add_tool(Box::new(
    WebFetchTool::new().with_max_content_length(10_000)
));
```

Good pattern:

1. search first with `WebSearchTool`
2. pick a relevant result
3. fetch the page with `WebFetchTool`
4. summarize or compare

See also: `docs/en/20-web-tools.md`

---

## 3. Browser Automation

Browser tools are not built in directly. They are typically provided through MCP, most commonly via Playwright MCP.

Common browser actions:

- `playwright_navigate`
- `playwright_snapshot`
- `playwright_click`
- `playwright_type`
- `playwright_screenshot`
- `playwright_close`

Use browser tools when:

- the page requires interaction
- content is rendered dynamically
- you need screenshots or page-state verification
- a plain fetch is insufficient

Use `WebFetchTool` instead when:

- the page is static
- you only need text
- no interaction is needed

See: `echo-agent-learning/examples/demo42_playwright_mcp.rs`

---

## 4. Data Tools

The built-in data tools are designed for local file analysis and transformation.

Common tools:

- `read_data` — read CSV / JSON / Parquet with preview
- `data_stats` — basic dataset statistics
- `filter_data` — filter rows by condition
- `aggregate_data` — grouped or global aggregations
- `transform_data` — sort / select operations
- `export_data` — export processed results

Typical use cases:

- inspect a CSV file before analysis
- compute summary stats for numeric columns
- filter rows matching a business rule
- group data by category and aggregate revenue

See: `echo-agent-learning/tests/example_contracts/demo43_data_tools.rs`

---

## 5. Office and Text File Tools

For document-style inputs, use the specialized file tools:

- `excel_info` — workbook / sheet overview
- `read_excel` — spreadsheet preview
- `read_word` — `.docx` content extraction
- `word_info` — Word document metadata
- `text_stats` — text statistics
- `search_text` — keyword search in text files

These are often a better first step than asking an LLM to reason from raw binary files.

---

## Recommended Workflow

For most real tasks, the best sequence is:

1. Use web search to discover sources.
2. Use web fetch or browser tools to retrieve details.
3. Use data tools to inspect structured files.
4. Let the Agent synthesize the results with `final_answer`.

If you need stronger reproducibility, prefer direct tool outputs over free-form LLM summaries.

---

## Related Docs

- `docs/en/02-tools.md`
- `docs/en/08-mcp.md`
- `docs/en/20-web-tools.md`
- `docs/en/14-semantic-search.md`
