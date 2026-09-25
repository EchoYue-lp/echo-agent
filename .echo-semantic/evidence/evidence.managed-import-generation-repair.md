---
schema_version: 1
id: evidence.managed-import-generation-repair
kind: evidence
observed_at: source:6fdcc1782b7aa2c97a148f7c377d60b4ed26b9d470f02c05a651de1e8e2eac74
source_refs:
  - echo-core/src/memory/conversation.rs
  - echo-state/src/memory/conversation.rs
  - echo-state/src/memory/file_conversation.rs
  - echo-state/src/memory/sqlite_conversation.rs
  - src/state/mod.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - tests/facade_smoke.rs
  - docs/adr/0080-managed-transcript-import-generation.md
supports: [behavior.context-memory-lifecycle, rule.context-persistence-separation]
limitations:
  - Host admission, cancellation, checkpoint repair ordering and UI projection remain consumer responsibilities
  - Framework File and SQLite backends are tested separately; EKO uses File only
---

# Managed import generation and runtime epoch repair

## 支持的结论

`ManagedConversationImport::prepare_for_generation` binds one transcript replacement to a runtime generation. File and SQLite Stores atomically replace visible rows, advance epoch, seed exact-row projection ordinals and retain the original import locator after restorable-message validation. Metadata-only updates leave that locator intact. Runtime cursors use normalized logical-message digests instead; the two digest meanings are deliberately separate.

`RuntimeCheckpointCasRequest.managed_import` carries the exact import and applied/idempotent receipt. File and SQLite runtime Stores permit a one-step active epoch transition only when the prior scope revision and runtime version still match and the checkpoint history/cursor exactly describes the imported rows. Ordinary cross-epoch CAS stays fenced. A consumer must use the persisted locator to replay the same import to recover a transcript commit that preceded its checkpoint CAS.

## 来源与范围

Focused File/SQLite tests cover seeded frontier continuation, unauthorized epoch fence, proof-bearing transition, mismatched checkpoint rejection and idempotent replay. `tests/facade_smoke.rs` compiles the public import and cursor surface. Full workspace and feature-gate results are recorded separately at delivery.

## 已知缺口

Host admission, cancellation, checkpoint repair ordering and UI projection remain consumer responsibilities. EKO's FileStore integration is verified in the application repository; this framework evidence alone does not close the consumer workflow.
