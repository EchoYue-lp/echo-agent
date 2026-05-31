# echo-tools

[![crates.io](https://img.shields.io/crates/v/echo_tools?color=brightgreen)](https://crates.io/crates/echo_tools)
[![docs.rs](https://docs.rs/echo_tools/badge.svg)](https://docs.rs/echo_tools)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Domain tools for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Features

| Feature | Tools | Dependencies |
|---------|-------|-------------|
| `files` | FileRead, FileWrite, FileDelete, FileList, FileDiff, FileEdit, FileGlob, FileGrep | default |
| `shell` | ShellTool (sandboxed command execution) | default |
| `web` | WebFetchTool, WebSearchTool, WebExtractTool + 3 search providers | scraper, html2text, url |
| `media` | ImageFetch, WebFetchEnhanced, PDF, Word, Excel | pdf-extract, lopdf, calamine, docx-rs |
| `chart` | ChartTool (vega-lite) | — |
| `data` | DataTool (polars-based analysis) | polars |
| `database` | DatabaseTool (sqlx-based queries) | sqlx |
| `git` | GitTool | — |
| `rag` | RAG retrieval tool | uuid |
| `research` | ArXiv search, Semantic Scholar, PDF fetch, BibTeX generation | reqwest, scraper |

## Usage

```toml
[dependencies]
echo_tools = "0.2"
```

## License

MIT
