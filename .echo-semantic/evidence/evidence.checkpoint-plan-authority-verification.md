---
schema_version: 1
id: evidence.checkpoint-plan-authority-verification
kind: evidence
observed_at: 733d352fc719f922b21bab1cd46206139564367f
source_refs:
  - src/agent/react/tests.rs
  - src/agent/react/run/stream_channel.rs
  - src/state/file.rs
  - src/state/sqlite.rs
  - README.md
  - README.zh.md
  - CHANGELOG.md
  - src/memory.rs
  - src/agent/snapshot.rs
  - echo-agent-learning/tests/documentation_contract.rs
  - docs/en/03-memory.md
  - docs/zh/03-memory.md
  - echo-agent-learning/examples/demo17_chat.rs
supports: [behavior.context-memory-lifecycle, behavior.task-subagent-execution, rule.context-persistence-separation, rule.task-subagent-authority]
limitations:
  - PR CI and main delivery remain pending
---

# Checkpoint plan authority verification

## 支持的结论

修复前 File 测试以 `ReAct republished a stale plan` 失败；修复后 File/SQLite
各自重新构造 Store 实例，旧值仍可读取，ReactAgent 恢复后新 checkpoint 的
`current_plan` 为 `None`，message history 保留。受管 ConversationStore 与
FileRuntimeStateStore 的 safe point 从旧值恢复后同步结算 transcript cursor；
File/SQLite pending intent proof-ack 测试各通过。

已有 channel 测试覆盖 A -> B -> A 切换后消息、技能及旧 plan 不串代，以及 B hydration
取消后旧值保持原样、A 路径继续成功。Bilingual memory/chat/persistence 文档与
demo17 编译示例不再宣称 ReAct 计划恢复。

## 来源与范围

- `cargo test -p echo_agent --features sqlite --lib checkpoint_legacy_plan_is_readable_but_not_republished --locked`: 2 passed.
- `cargo test -p echo_agent --features sqlite --lib managed_force_checkpoint_settles_transcript_before_returning --locked`: 1 passed.
- `cargo test -p echo_agent --features sqlite --lib pending_transcript_requires_proof_carrying_acknowledgement --locked`: 2 passed.
- `warm_agent_switches_runtime_state_identity_without_context_leakage` 与
  `cancelled_runtime_switch_cannot_publish_partial_hydration` 的 focused
  运行各 1 passed，并由最终完整 workspace 测试再次覆盖。
- `cargo check -p echo-agent-learning --example demo17_chat --locked`: passed.
- `cargo test -p echo-agent-learning --test documentation_contract public_checkpoint_docs_do_not_claim_plan_recovery --locked`: 1 passed.
- `./scripts/verify.sh`: passed after the final source edit, covering fmt,
  both Clippy policies, workspace all-target/all-feature tests, and root
  no-default-features library check.
- Repository-required isolated feature matrix passed all 17 features:
  `acp`, `a2a`, `mcp`, `lsp`, `sqlite`, `telemetry`, `topology`,
  `subagent`, `web`, `media`, `data`, `statistics`, `channels`,
  `git`, `database`, `rag`, and `chart`.
- Strict semantic snapshot plus change-evidence verification passed against
  `be8cbbc6e95c8082814e3cb5fad9823a7cbf42a9`.

## 已知缺口

独立复审在当前 semantic source 上 PASS，行动项为 0。PR CI 及远端 main
交付的结果待补；外部 Issue 在这些步骤完成前保持 open。
