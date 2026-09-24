---
schema_version: 1
id: evidence.memory-provenance-authority-verification
kind: evidence
observed_at: source:757f499d4d9a40a4c27791933cb3d5e9d3b2dda76a1a28e4561e317d4719be94
source_refs:
  - echo-core/src/memory/types.rs
  - echo-state/src/compression/mod.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/phases/compact.rs
  - src/evolution/layer.rs
  - src/evolution/recall.rs
  - src/evolution/review.rs
  - src/evolution/dreaming.rs
  - src/memory_promoter.rs
  - src/evolution/triggers.rs
  - src/tools/builtin/memory.rs
  - src/agent/react/tests.rs
  - echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs
  - echo-agent-learning/tests/example_contracts/demo51_self_improvement.rs
  - echo-agent-learning/examples/demo18_semantic_memory.rs
  - echo-agent-learning/examples/demo27_sqlite_memory.rs
  - echo-agent-learning/examples/demo45_customer_service.rs
  - docs/en/03-memory.md
  - docs/zh/03-memory.md
  - docs/en/25-self-improvement.md
  - docs/zh/25-self-improvement.md
supports: [finding.pre-compaction-memory-trust-provenance, behavior.eval-evolution, behavior.context-memory-lifecycle, rule.quality-observation-boundary, rule.context-persistence-separation]
limitations:
  - Failure injection covers journal/audit/caller cancellation rather than physical power loss
---

# Issue 76 memory provenance verification

## 支持的结论

旧实现上两个 red 测试分别因无来源的模型断言被保存和 Draft/legacy Active 进入 recall
退出 101。修复后 scoped 测试覆盖正常 pre-compaction 提取、缺证据、tool quote
冒充 user、user/assistant/tool mixed Draft 与 High 风险、LLM failure/noop。
Layer tests 覆盖显式激活、伪造 Active、重复抽取保留 approval、A→B→A、
取消后对账、audit 失败重启、File hot provenance 往返、旧 hot 审阅隔离、
SQLite Store 重启、含密钥证据拒绝及新事实 telemetry 清零。
Recall tests 覆盖 Draft/旧 Active 隔离、approved Active/Archived、Draft 密集搜索
不遮蔽 approved 记录、并发 Draft 不被旧 telemetry CAS 覆写。Dreaming 对 Draft
与未批准旧计数不作状态晋升。Demo51 在 `eval,improve` feature 下执行
Draft→approval→Hot→restart 的公开 API 流程。
新增回归证明无 manager 的 Agent 工具只返回已批准 typed 值，Assistant 来源偏好
无法激活；Hot 晋升后仍自动进入上下文，合成投影/Horizon 消息不会冒充用户证据，
长工具结果的证据保持精确且有界，Horizon 提升失败保留完整消息历史。
Store→manager 与 manager→Store 两个安装顺序，以及 context 锁忙时的失败，
都验证公开 `store()`、工具与 manager 不会分叉。

## 来源与范围

- `cargo test -p echo_core memory::types::tests --locked`: 15 passed.
- `cargo test -p echo_agent --lib --locked`: 727 passed before the last synthetic-source regressions; those regressions passed separately.
- `cargo test -p echo_state horizon_promotion_failure_restores_original_messages --locked`: 1 passed.
- `cargo test -p echo_agent --lib pre_compaction_flush_rejects_framework_context_as_user_evidence --locked`: 1 passed.
- `cargo test -p echo_agent --lib promoter_does_not_attribute_framework_context_to_user --locked`: 1 passed.
- `cargo test -p echo-agent-learning --features eval,improve --test example_contracts contract_demo31_memory_tools --locked`: 1 passed.
- `cargo check -p echo-agent-learning --example demo18_semantic_memory --features web --locked`: passed.
- `cargo check -p echo-agent-learning --example demo27_sqlite_memory --features sqlite --locked`: passed.
- `cargo check -p echo-agent-learning --example demo45_customer_service --features sqlite,human-loop,content-guard --locked`: passed.
- `cargo test -p echo_agent --lib layer_manager_install --locked`: 1 passed.
- `cargo test -p echo_agent --lib busy_context_rejects_manager_install_without_partial_publication --locked`: 1 passed.
- `./scripts/verify.sh`: exit 0, covering fmt check, both workspace all-feature Clippy gates, all-target/all-feature workspace tests, and no-default-features lib check.
- `for feature in acp a2a mcp lsp sqlite telemetry topology subagent web media data statistics channels git database rag chart; do CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=4 cargo check -p echo_agent --no-default-features --features "$feature" --locked || exit 1; done`: 17/17 passed, exit 0.
- `cargo test -p echo_agent --features sqlite --lib --locked`: 740 passed.
- `cargo test -p echo_agent --features sqlite --lib evolution::layer::tests --locked`: 52 passed.
- `cargo test -p echo_agent --features sqlite --lib pre_compaction_flush --locked`: 6 passed before the final mixed-role regression; the full SQLite lib run covers the later source.
- `cargo test -p echo-agent-learning --features eval,improve --test example_contracts contract_demo51_layered_memory_recovery_api --locked`: 1 passed.

原始默认 feature learning 命令执行 0 个 demo51 测试，已改用实际启用模块的命令验证。
第一次完整门禁在 `echo_state` 的 collapsible-if Clippy 告警处停止；修复后重新执行
整个脚本至 exit 0，未以 focused 测试替代失败门禁。

## 远端交付与合并后验证

PR #152 的 Rust CI run `35961491513` 七项 job 全部 success：Linux quality，
framework/foundations/tools/learning 分组测试，Windows compile/atomic replacement，
dependency policy。squash commit `5a0f2af2da8de9db2bf98c3aa8dd2a54e1152d7c`
进入 framework main，GitHub commit verification 为 valid；该 main 树与通过本地
完整门禁的候选树无差异。独立 worktree 从已合并 main 检出后执行 strict semantic
snapshot，exit 0，source digest 仍为本 Evidence 的 `observed_at`。

## 已知缺口

独立 rereview PASS，见 `audit.memory-provenance-authority-rereview`。外部
SDK/CLI/website/A2A 不在本 Finding 完成范围。
