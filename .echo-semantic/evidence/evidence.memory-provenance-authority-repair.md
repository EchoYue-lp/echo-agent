---
schema_version: 1
id: evidence.memory-provenance-authority-repair
kind: evidence
observed_at: source:87b717676a7b51e213630677989947777b4bed441acd8c7d655fb6c96dca77ad
source_refs:
  - echo-core/src/memory/mod.rs
  - echo-core/src/memory/types.rs
  - echo-state/src/memory/mod.rs
  - echo-state/src/memory/typed_store.rs
  - echo-state/src/compression/mod.rs
  - src/lib.rs
  - src/agent/config.rs
  - src/agent/react/builder.rs
  - src/agent/react/mod.rs
  - src/evolution/layer.rs
  - src/evolution/recall.rs
  - src/evolution/review.rs
  - src/evolution/dreaming.rs
  - src/evolution/security.rs
  - src/memory_promoter.rs
  - src/agent/react/run/context.rs
  - src/agent/react/run/phases/compact.rs
  - src/evolution/background_review.rs
  - src/evolution/triggers.rs
  - src/tools/builtin/memory.rs
  - echo-agent-learning/tests/example_contracts/demo31_memory_tools.rs
  - echo-agent-learning/examples/demo18_semantic_memory.rs
  - echo-agent-learning/examples/demo27_sqlite_memory.rs
  - echo-agent-learning/examples/demo45_customer_service.rs
  - docs/adr/0070-memory-provenance-and-recall-authority.md
supports: [finding.pre-compaction-memory-trust-provenance, behavior.eval-evolution, behavior.context-memory-lifecycle, rule.quality-observation-boundary, rule.context-persistence-separation]
limitations:
  - Caller identity in MemoryApproval is owned by the embedding host
  - Raw Store readers may observe ADR 0065 prepared projections before manager reconciliation
---

# Issue 76 memory provenance authority repair

## 支持的结论

原 pre-compaction LLM 和 evicted-message promoter 把混合来源文本直接写成 Active；
现有 `MemorySource` 只能表达生成机制，无法证明 speaker trust。修复在公开
`MemoryMeta` 中增加 serde-default provenance，保存来源角色、精确证据及调用方批准
receipt。旧 typed/Hot 记录默认 LegacyUnknown，原始 Store/manager 仍可读取审阅，
但不能进入模型 context。新自动 writer、trigger、layered remember 和 Background
Review 保存 Draft；pre-compaction 引用在写入前与原始 user/assistant/tool 消息
逐项匹配，用户偏好只接受 user evidence。Manager 拒绝含密钥或指令式证据，
将 trust-based security verdict 的风险写回 metadata。
Context projection、Hook/runtime note、Horizon 合成摘要与缺失工具结果占位符虽
可能以 User/Assistant/Tool role 呈现，却在提取前排除，不能冒充原始证据。
Trigger 的证据只保留来源字段中的原文片段，不拼接人工省略号或 session 描述。

`MemoryLayerManager` 是唯一 Draft-to-Active 持久 mutation owner。
`write_memory` 即使收到外部伪造的 Active/approval 也只写 Draft；重复抽取与已批准
内容相同则保留原事实。审阅预览绑定完整 content、metadata 和 journal generation。
`activate_draft` 在原 operation lock 内核对原值与 generation，经 ADR 0065 的
prepare/project/audit/settle 提交；A→B→A、stale reviewer、不同 receipt 重放在
任何副作用前失败。失败或取消后由同一 journal 对账，只有原激活仍为 key 的最新
lineage 时相同批准才返回 AlreadyActivated。新 Draft 的 recall 计数不继承旧
Active 事实。

`MemoryMeta::is_recallable` 是 warm/hot 的共同准入：只接受有精确证据和显式批准的
Active/Archived；UserPreference 还必须有真实 User 证据。自动 ReAct 召回先经
manager 对账，加载全部合格 Hot，再使用与 layered tool 相同的 `MemoryRecaller`
获取相关 Warm。Agent 自带的 Store-backed recall/search 也共用该过滤器；无 manager
时不注册写入工具，安装后才由 manager 提供 journaled remember/forget。
安装 manager 时公开 `store()` 同步绑定其底层 Store；同步忙时与已有 manager
不匹配的新 Store 均显式失败，不发布部分工具/持久 owner 配置。
候选窗口可扩张以免 Draft 淹没 approved 结果，返回前再读取
当前状态。Recall telemetry 用 Store CAS，不能覆盖并发修改的 status/provenance。
Dreaming 不用 Draft 或未证实的历史计数自行激活记忆。Horizon 提升失败会恢复
原消息快照，避免错误路径清空上下文。

## 来源与范围

Framework 变更始于 `origin/main@c45f83cbed1722f86d3d99f0f7e2edd2ff105016`，
使用独立 `fix/Echoyue/memory-provenance-authority` worktree。公共类型与
File/SQLite Store 格式保持可解码；无第二 Store、审批 UI、Task 状态或 schema
migration。ADR 0070 记录行业参考、候选方案、分层选择和兼容影响。

## 已知缺口

Raw `Store` 是通用 KV API，不为任意 JSON 声明 typed memory 审批语义。外部
代码可以直接写底层 Store；需要已结算的框架记忆读取时应使用
`MemoryLayerManager`。完整门禁和逐 feature matrix 已通过，PR #152 七项 CI 全绿，
修复进入 framework `main@5a0f2af2da8de9db2bf98c3aa8dd2a54e1152d7c`。
独立复审结论见 `audit.memory-provenance-authority-rereview`：本候选
Critical/Important/Minor 为 0。
