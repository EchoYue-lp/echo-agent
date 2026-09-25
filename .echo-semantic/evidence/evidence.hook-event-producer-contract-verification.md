---
schema_version: 1
id: evidence.hook-event-producer-contract-verification
kind: evidence
observed_at: source:28f42a6c8a446593492907c8e09effd43a986a5665776e38448b09fcba7fc585
source_refs:
  - tests/hook_event_producer_contract.rs
  - docs/en/23-hooks.md
  - docs/zh/23-hooks.md
  - docs/en/07-skills.md
  - docs/zh/07-skills.md
  - docs/adr/0073-hook-event-producer-contract.md
supports: [finding.hook-event-producer-contract]
limitations:
  - PermissionDenied remains reserved for the #37 approval authority boundary
  - host-owned task/evolution adapters require embedding application wiring
command_results:
  - { command: "cargo test --test hook_event_producer_contract --locked", exit_code: 0 }
  - { command: "cargo test --test documentation_contract --locked", exit_code: 0 }
  - { command: "cargo fmt --all -- --check", exit_code: 0 }
  - { command: "git diff --check", exit_code: 0 }
---

# HookEvent producer contract verification

## 支持的结论

The exhaustive integration test covers all 31 HookEvent names and requires the
English and Chinese matrices to classify each name exactly once with identical
ownership. Documentation contract tests and focused lint/format checks pass.

## 来源与范围

验证针对当前 framework source snapshot，覆盖 bilingual matrix、ADR 与 executable
contract test；PermissionDenied 和 host-owned adapters 保留各自边界。

## 已知缺口

PermissionDenied 的 exactly-once producer 由 #37 单独负责；host-owned adapters
仍需 embedding application 接线。
