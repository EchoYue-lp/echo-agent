# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `SandboxStreamEvent::Failed` and `SandboxStreamFailure` expose cancellation,
  output-drain, and cleanup debt as a typed live-stream terminal instead of an
  unexplained EOF or successful-looking completion.

- **HookAction::ActivateSkill**: 声明式直接激活技能 hook 动作。frontmatter 可写
  `type: activate_skill`，execute_action 产出 `HookResult.activate_skill`。
- **fire_lifecycle_hook 接线**: 收到 `HookResult.activate_skill` 后调用
  `activate_skill_for_context` 完成真实激活，reason 注入 runtime note。
- **UserPromptSubmit 每轮触发**: `prepare_stream_context` /
  `prepare_react_context` 每轮传用户输入触发 hook，支持 content-based
  matcher（如 `\\.docx` glob 匹配）。
- **hook_activation_cache**: `Arc<Mutex<Option<(String,String)>>>` 共享槽位，
  由 P1 prepare 阶段写入、P4 TriggerSupervisor 消费。
- **SkillLoader hooks.json 发现**: `scan_directory` 并列读取 SKILL.md 同级
  `hooks.json`（EKO 格式），合并进 descriptor.hooks。
- **resolve_python uv 优先**: `uv run --script` → python3 → python 三级回退，
  PEP 723 内联依赖自动处理。
- **minimal_env +HOME**: 白名单加 HOME，本地桌面 agent 脚本可读 `~/.config`。
- **SkillSandboxPolicy 接线**: `apply_sandbox_policy` 翻译 timeout，
  `sandbox_limits_from_policy` 翻译 network/allowed_paths 到 ResourceLimits，
  通过 `execute_with_limits` 实现 OS 级隔离。
- **默认 sandbox local_only**: `ReactAgentBuilder::build()` 默认装配
  `SandboxManager::local_only()`，不再裸跑。
- **dependency_probe 模块**: 从 SKILL.md metadata 提取 `requires-binaries` /
  `requires-python-packages`，生成结构化 `ProbeReport`。
- **SkillRegistry::inject_methodology_baseline()**: 对 category=methodology
  且 enabled baseline 的技能，从磁盘读 SKILL.md 并注入 system prompt。
- **DEFAULT_BASELINE_SKILLS**: brainstorming / systematic-debugging /
  verification-before-completion / writing-plans。
- **TriggerSupervisor 三源融合引擎**: 实现 `IntentClassifier` trait，
  Keyword（0 token）+ LLM（可选）+ Hook slot 三源融合，`fuse()` 纯函数可单测。

### Changed

- Local and Docker sandbox execution now transfer spawned resources to detached
  backend owners. Unix Local cancellation captures and verifies the process
  group; Windows Local execution is unavailable until Job Object ownership is
  implemented. Docker uses a caller-abandonment guard, unique container name,
  bounded CLI control stages and output readers, and reaches checked
  `docker rm -f` cleanup after normal, non-zero, timeout, cancellation, stdin
  failure, invalid create output, and caller abort paths. Reserved isolation and
  ownership flags cannot be supplied through `extra_args`.

- Channel `SessionHandler` now isolates Agent state, locks, mode, HITL,
  timeout, and reset by `(channel_id, conversation_id, sender_id)`. Malformed
  identities are rejected before an Agent is created, and built-in QQ/Feishu
  adapters no longer emit a shared `unknown` sender sentinel. Feishu scopes
  `open_id` and `user_id` in distinct identity namespaces.

- Procedural macros now resolve their owning split crate directly:
  core-owned macros accept `echo_core`, while `#[handler]` accepts
  `echo_orchestration` plus `echo_core`; neither requires the facade package.

- `HookResult` 新增 `activate_skill: Option<(String, String)>` 字段 +
  `with_activate_skill` 构造函数。
- `HookAction` enum 新增 `ActivateSkill` 变体（validate/execute_action/merge）。
- `minimal_env` 白名单 +HOME。
- `ReactAgent` 新增 `hook_activation_cache` 字段 + public getter。
- `SandboxManager` 默认 `local_only()`。
- Task graph mutation and execution now have one authority:
  `TaskRevisionService` owns CRUD and relation commits, while
  `RuntimeTaskService` owns ready-frontier, retry, cancellation, and terminal
  settlement. The legacy `ManagedTask`/`PlanSpec`/`Verifier` parallel model was
  removed; product fields round-trip through `TaskSpec::extension`.
- `TaskToolPolicy` implementations must now provide the idempotent
  `abort_scope_preparation` hook. `TaskRevisionService::create_from_tool`
  invokes it after any post-`ensure_scope` preparation, validation, load, or
  commit failure so product adapters can discard unpublished scope resources
  without duplicating framework DAG validation. A failing `ensure_scope` must
  clean its own unpublished side effects before it returns.
- Team collaboration can be declared by registered names with `TeamSpec`, or
  composed from concrete Agent instances with `TeamAgentBuilder`. Programmatic
  members are registered into the same `SubagentRegistry` and executed by the
  same `SubagentExecutor`; both entry points compile into the revisioned Task
  graph and run through `RuntimeTaskService`.
- `JsonlChangeLog::new` and `MemoryRuntimeIntegrationBuilder` initialization
  now return `Result` and fail closed on complete-record corruption. Stable
  audit IDs can be replayed through `ChangeLog::record_idempotent`; identical
  entries are not duplicated and ID collisions with different content fail.
- Dropping an Agent event stream now cancels the run and lets already-started
  tools reach their bounded terminal safe point before a reaper can hard-abort
  the run, preserving durable tool cleanup without leaking the active turn.
- Documentation no longer references the deprecated `Checkpointer` API across
  `README{,.zh}.md`, `echo-core/README.md`, `echo-state/README.md`, and the
  `docs/{en,zh}/` guides. The memory guide now documents
  `RuntimeStateStore`, `ConversationStore`, and `Store` as separate layers.
- Doc comments in `src/memory.rs`, `src/agent/config.rs`,
  `src/agent/snapshot.rs`, and `echo-state/src/memory/store.rs` no longer
  reference the legacy checkpoint trait.

### Removed

- Removed the safe `PluginVariables::export_to_env` API, whose process-global
  environment mutation required an unenforceable single-threaded precondition.
  Plugin consumers should use `PluginVariables::substitute` or pass explicit
  environment entries to the subprocess they own.

- **`Checkpointer` trait and its implementations** (`FileCheckpointer`,
  `InMemoryCheckpointer`) are gone from the source tree. The trait was
  already absent from public re-exports as of 0.2.0, and its setters were
  no-ops on `ReactAgent`. New code should use:
  - [`RuntimeStateStore`](src/state/mod.rs) — ReAct runtime checkpoint
    (messages + current plan + active skills + blocked reason) for crash
    recovery; concrete implementations: `FileRuntimeStateStore` and the
    optional `SqliteRuntimeStateStore`. Revisioned task graphs are persisted
    separately by the canonical task runtime.
  - `ConversationStore` — user-visible transcript projection;
    concrete implementations include `FileConversationStore` and the optional
    `SqliteConversationStore`.
- The parallel legacy Task authority was removed: `TaskManager`, `TaskStore`,
  `SqliteTaskStore`, `TaskExecutor`, `TaskScheduler`, composite execution,
  Task hooks, and replanners. Migrate graph CRUD to `TaskRevisionService`, DAG
  execution to `RuntimeTaskService`, and independent background futures to
  `TaskSpawner`.
- Runtime-state `TaskNode` / `TaskNodeStatus` APIs were removed. ReAct
  `RuntimeStateStore` checkpoints now contain only resumable Agent state;
  revisioned task progress remains in the canonical Task graph.
- The Team-specific `TaskNode` checkpoint format and manager-owned ready/fan-out
  loop were removed. Dynamic manager plans are committed as a new revision by
  `TaskRevisionService`, then executed by `RuntimeTaskService`. The public
  `Team`, `TeamMember`, `TeamRole`, `TeamAgentBuilder`, and `TeamStrategy`
  composition APIs remain available as thin adapters. Shared Agent objects now
  enter directly through `register_shared` / `add_shared_member`; the obsolete
  `ArcAgentBox` wrapper is not restored.
  `TeamRuntime` and `execute_team_on_runtime` let persistent consumers reuse
  their canonical revision/result authority. `TeamAgent` retains its in-memory
  runtime only when given an explicit stable run ID; anonymous executions do
  not accumulate old graphs.
- Manager-produced task plans now use a strict typed JSON contract. Invalid
  schemas, unknown Subagents, duplicate task IDs, and invalid dependencies fail
  closed before the plan is committed as a graph revision.
- The historical `TeamStrategy::Swarm.batch_size` field was removed because it
  was never read by the executor. A batching option should only return with an
  implemented graph-partitioning contract.

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
- Multi-agent orchestration (Subagent, Handoff, Plan-and-Execute, Self-Reflection)
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
