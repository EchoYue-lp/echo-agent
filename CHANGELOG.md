# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
