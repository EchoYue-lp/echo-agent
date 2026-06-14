# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Removed

- **`Checkpointer` trait and its implementations** (`FileCheckpointer`,
  `InMemoryCheckpointer`) are gone from the source tree. The trait was
  already absent from public re-exports as of 0.2.0, and its setters were
  no-ops on `ReactAgent`. New code should use:
  - [`RuntimeStateStore`](src/state/mod.rs) — full runtime checkpoint
    (messages + current plan + active skills + blocked reason + TaskNode
    DAG) for crash recovery; concrete implementation:
    `SqliteRuntimeStateStore`.
  - `ConversationStore` — user-visible transcript projection;
    concrete implementation: `SqliteConversationStore`.

### Changed

- Documentation overhaul: removed all references to the deprecated
  `Checkpointer` API across `README{,.zh}.md`, `echo-core/README.md`,
  `echo-state/README.md`, and the `docs/{en,zh}/` guides. The memory
  chapter (`docs/{en,zh}/03-memory.md`) is now organized around the three
  layers: `RuntimeStateStore`, `ConversationStore`, and `Store`.
- Doc-comments in `src/memory.rs`, `src/agent/config.rs`,
  `src/agent/snapshot.rs`, and `echo-state/src/memory/store.rs` no longer
  reference the legacy trait.

## [0.2.0] — 2026-05-29

### Added

- **Research tools** — 4 new tools for academic paper workflows (feature flag: `research`)
  - `arxiv_search`: Search ArXiv API for preprints (Atom/XML parsing, category filtering)
  - `semantic_scholar_search`: Search Semantic Scholar for published papers (citation counts, fields of study)
  - `pdf_fetch`: Download and parse PDF documents from URL (page range, metadata extraction)
  - `bibtex_generate`: Generate BibTeX entries from paper metadata (arXiv ID → primaryClass extraction, cite key disambiguation)
- **Tool permission system** — all 67 tools now declare `ToolPermission` (Read/Write/Network/Execute/Sensitive)
- **SSRF protection for research tools** — `arxiv_search`, `semantic_scholar_search`, `pdf_fetch` all use safe redirect policy and private IP blocking

### Fixed

- **Security: ArXiv API URL** — changed from `http://` to `https://export.arxiv.org/api/query`
- **Security: URL encoding** — arxiv query/category and semantic_scholar query/fields_of_study are now URL-encoded to prevent parameter injection
- **Security: MySQL escaping** — fixed single-quote escaping from `"\\'"` to `"''"` (standard SQL)
- **Security: Table name validation** — added regex check for alphanumeric/underscore/dot only
- **Security: Database URL validation** — only sqlite/mysql/postgresql/postgres schemes accepted
- **Security: SQL blacklist expanded** — added `EXECUTE`, `EXEC`, `INTO OUTFILE`, `LOAD_FILE`
- **Security: Image tools permissions** — `ImageAnalysisTool` and `ImageFetchTool` now declare `ToolPermission::Network`
- **Security: All data/excel/chart/word/pdf/rag tools** — permissions added to all 30+ tools
- **PDF double-parse eliminated** — `pdf_fetch` now parses PDF once for both text extraction and metadata
- **BibTeX primaryClass extraction** — fixed old-format arxiv IDs (`cs.AI/1234567`) and added new-format fallback via `fields_of_study`
- **BibTeX cite key disambiguation** — duplicate author-year combos now get `a`, `b`, `c` suffixes

### Changed

- **Shared HTTP client pattern** — research tools use `OnceLock<reqwest::Client>` instead of creating fresh clients per request
- **`/compact` vs `/compress` separation** — `/compact` now uses lightweight `force_compress(12)` (keeps more recent messages), `/compress` uses full `force_compress(6)`

## [0.1.4] - 2026-05-09

### Changed

- **Architecture: trait and data types moved from facade to echo_core** — echo_core is now the true abstraction layer (22 new files, +2,772 lines). All core traits (Agent, Planner, Executor, Critic, ContextCompressor, Store, Checkpointer, etc.) and their data types are defined in echo_core with zero external dependencies.
- **New `echo_tools` crate** — all domain tools extracted from facade into independent crate (33 files, +12,094 lines). New tools: ToolRegistry, FileDiff, FileEdit, FileGlob, FileGrep, WebExtract.
- **Facade trimmed by ~13,000 lines** — deleted proxy modules, retained only heavy implementations + re-exports + PlanExt extension trait.
- **SummaryCompressor API simplified** — `new(llm, keep_recent)` no longer requires explicit prompt parameter; `with_prompt()` for custom prompts.
- **Bumped all 8 workspace crates to 0.1.4**

### Fixed

- **`content-guard` feature was a no-op** — it did not activate `echo_core/guard`. Now correctly propagates: `content-guard = ["echo_core/guard"]`.
- **Feature propagation chain was broken** — `mcp`, `channels`, `web`, `media`, `data`, `git`, `database`, `rag`, `chart`, `shell`, `files` features did not propagate to their respective echo_* crate features. All now correctly chained.
- **`testing` module visible in production builds** — now gated behind `#[cfg(any(test, feature = "testing"))]`. 7 examples given `required-features = ["testing"]`.
- **McpError / ChannelError unconditionally compiled into ReactError** — now feature-gated (`#[cfg(feature = "mcp")]` / `#[cfg(feature = "channels")]`).
- **7 Clippy warnings fixed** — 3× type_complexity (type aliases), 2× incompatible_msrv (% n == 0), 1× unnecessary_filter_map, 1× needless_range_loop.
- **Documentation fixed** — missing `.await` on `set_compressor()`, missing `Box::new` in `ReactError::Llm()`, `force_compress_with` temporary lifetime, wrong example filename, added Builder pattern guidance.

## [0.1.3] - 2026-05-01

### Fixed

- Fixed `skills/external` and `skills/hooks` module paths not resolving in published crate
- Fixed `"skills/"` exclude pattern in Cargo.toml accidentally excluding `src/skills/`
- Fixed docs.rs build failure caused by `default = ["full"]` pulling in polars on nightly
- Fixed CI runner OOM by limiting build parallelism and adding swap
- Fixed CI swap file creation conflict (ETXTBSY)

## [0.1.2] - 2026-04-30

### Fixed

- Fixed docs.rs build failure by adding `rust-version`, `[package.metadata.docs.rs]`, and `rust-toolchain.toml`
- Fixed incorrect crate name in README badges (`echo-agent` → `echo_agent`)
- Added missing `exclude` field to reduce package size from 2.47 MB
- Added missing Cargo.toml metadata (`keywords`, `categories`, `documentation`, `homepage`, `readme`, `rust-version`)

### Changed

- Added `#![doc = include_str!("../README.md")]` to lib.rs for rich docs.rs landing page
- Added docs.rs and CI badges to README
- Bumped version to 0.1.2

## [0.1.1] - 2026-04-29

### Changed

- Bumped workspace crate versions to 0.1.1

## [0.1.0] - 2026-04-29

### Added

- Initial release of echo-agent framework
- ReAct engine with Thought → Action → Observation loop
- Multi-agent orchestration (SubAgent, Handoff, Plan-and-Execute, Self-Reflection)
- Dual-layer memory (Store + Checkpointer)
- Tool system with `#[tool]` macro
- Context compression (SlidingWindow / Summary / Hybrid)
- MCP protocol client (stdio / SSE / HTTP)
- A2A protocol (Agent Card, task lifecycle, streaming)
- IM channels (QQ Bot, Feishu)
- Graph workflow engine
- Declarative workflow (YAML/JSON)
- Guard system (Rule + LLM)
- Sandbox execution (Local / Docker / K8s)
- OpenTelemetry integration
- 40+ examples and 6 comprehensive demos
- Bilingual documentation (EN + ZH)

[0.2.0]: https://github.com/EchoYue-lp/echo-agent/compare/v0.1.4...v0.2.0
[0.1.4]: https://github.com/EchoYue-lp/echo-agent/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/EchoYue-lp/echo-agent/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/EchoYue-lp/echo-agent/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/EchoYue-lp/echo-agent/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/EchoYue-lp/echo-agent/releases/tag/v0.1.0
