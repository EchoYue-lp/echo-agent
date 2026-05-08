# echo_tools

Domain tools for the [echo-agent](https://github.com/EchoYue-lp/echo-agent) framework.

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

## Usage

```toml
[dependencies]
echo_tools = "0.1"
```

## License

MIT
