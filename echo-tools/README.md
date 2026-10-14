# echo-tools

[![crates.io](https://img.shields.io/crates/v/echo_tools?color=brightgreen)](https://crates.io/crates/echo_tools)
[![docs.rs](https://docs.rs/echo_tools/badge.svg)](https://docs.rs/echo_tools)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Domain tools for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Features

| Feature | Tools | Dependencies |
|---------|-------|-------------|
| `artifact` | Verified, paged reads of spilled tool output | sha2 |
| `files` | Artifact reader plus FileRead, FileWrite, FileDelete, FileList, FileDiff, FileEdit, FileGlob, FileGrep | tree-sitter, encoding_rs |
| `shell` | ShellTool (sandboxed command execution) | none |
| `web` | WebFetchTool, WebSearchTool, WebExtractTool + 3 search providers | scraper, html2text, url |
| `media` | ImageFetch, WebFetchEnhanced, PDF, Word, Excel | lopdf, calamine, docx-rs |
| `chart` | ChartTool (vega-lite) | — |
| `data` | DataTool (polars-based analysis) | polars |
| `database` | DatabaseTool (sqlx-based queries) | sqlx |
| `git` | GitStatus, GitDiff, GitLog, GitBlame, GitBranch, GitCommit | — |
| `rag` | RAG retrieval tool | uuid |
| `research` | Paper search, PDF/BibTeX tools, OpenAlex/Crossref/Europe PMC/Zotero clients | reqwest, quick-xml |

## Usage

```toml
[dependencies]
echo_tools = "0.2"
```

## License

MIT

### Git branch switching

`git_branch` with `switch` selects an existing branch using native `git switch`,
with the same repository/working-directory context as the other Git tools. It
preserves compatible uncommitted edits, including a tracked file with the same
name as the branch. Conflicting edits, missing/invalid branches and option-like
targets fail explicitly; it never forces a checkout or restores a path under
the guise of switching branches. Create/delete/list behavior is unchanged.
