---
schema_version: 1
id: evidence.high-risk-audit-frontier
kind: evidence
observed_at: f1e9027246760661144786e9e35615cd46d580c6
source_refs:
  - Cargo.toml
  - echo-orchestration/src/runtime/turn_driver.rs
  - src/agent/snapshot.rs
  - echo-orchestration/src/tasks/revisioned.rs
  - src/agent/subagent/registry.rs
  - echo-orchestration/src/workflow/graph.rs
  - echo-state/src/journal/mod.rs
  - src/agent/react/run/pipeline.rs
  - echo-integration/src/mcp/client.rs
  - src/plugin/prepared.rs
  - echo-integration/src/providers/client.rs
  - src/a2a/server.rs
  - src/eval/runner.rs
  - src/evolution/layer.rs
supports: [behavior.workspace-composition, behavior.agent-turn-lifecycle, behavior.context-memory-lifecycle, behavior.task-subagent-execution, behavior.observation-persistence, behavior.effect-permission-execution, behavior.extension-publication, behavior.llm-provider-execution, behavior.protocol-projection, behavior.eval-evolution, rule.framework-layer-ownership, rule.turn-terminal-authority, rule.context-persistence-separation, rule.task-subagent-authority, rule.fact-projection-separation, rule.permission-effect-order, rule.extension-generation-authority, rule.provider-protocol-boundary, rule.protocol-role-separation, rule.quality-observation-boundary]
limitations:
  - 只归并当前source digest上的审计结论，不授权repair、不关闭Finding、不代表外部环境或远端CI已验证
  - decision_required项必须先按AGENTS.md调研成熟实现并取得人的明确裁决
  - 同一Finding含可拆工程缺口和产品保证时可进入两个frontier，但实施必须拆成独立可交付Plan
---

# High-risk Audit Repair Frontier

## 支持的结论

26份全workspace定向Audit覆盖10个顶层边界，另保留1份既有SDK专项Audit。当前91个Finding中86个open、5个resolved；所有open Finding都至少引用一份当前source digest的Audit。Frontier只决定下一步路由，不把审计通过写成问题已解决。

## 来源与范围

审计按“boundary × risk lens × source digest”执行，每轮不超过三个单元；覆盖state authority、data durability、time lifecycle、failure concurrency、permission/external effect和contract evidence。下列分组按独立修复与裁决依赖组织，不按API identity数量排序。

## Decision Required

| 决策簇 | Finding | 进入设计前唯一问题 |
| --- | --- | --- |
| Turn commit 与 adapter | `turn-driver-entry-coverage`, `agent-adapter-close-settlement`, `turn-terminal-commit-projection-order` | 哪个时点提交Turn终态，以及Channel/Headless/A2A adapter何时必须使用driver并await close？ |
| A2A/Channel surface | `a2a-advertised-capability-binding`, `a2a-terminal-authority`, `channel-attachment-projection`, `channel-reset-stale-generation-delivery` | A2A是否限定text+SSE或补file/push，Channel是否multimodal，reset是否fence旧代？ |
| Scheduler delivery | `scheduler-cache-delivery`, `scheduler-control-fire-race` | missed occurrence、失败retry、disable后已admit occurrence与持久invocation identity的保证是什么？ |
| Hook permission | `hook-permission-precedence` | source precedence是否可覆盖global deny，还是deny必须跨所有source胜出？ |
| Plugin publication | `plugin-failure-isolation-contract` | 整个PreparedPluginSet原子发布，还是按Plugin/组件隔离健康部分？ |
| LSP handle | `lsp-manager-derived-handle-resurrection` | 派生client在manager close后级联失效，还是拥有独立生命周期？ |
| Structured output/provider policy | `structured-output-main-path`, `structured-output-schema-validation-contract`, `provider-capability-authority`, `model-fact-freshness-authority` | response_format作用阶段、strict保证、capability owner和动态facts刷新责任是什么？ |
| Observation policy | `diagnostic-persistence-failure-visibility`, `trace-audit-secret-boundary` | trace/audit是best-effort或required，以及各backend的failure/redaction/retention合同是什么？ |
| Event/Guard public contract | `workflow-entry-loop-drift`, `trace-effect-event-producers`, `hook-event-producer-contract`, `guard-direction-contract` | NodeError/Token与Run/Hook/Guard变体应补真实producer/error语义，还是退役、收窄或重命名公共合同？ |
| Task/Workflow public contract | `subagent-definition-catalog`, `workflow-dag-authority`, `checkpoint-current-plan-orphan-authority` | definition catalog是否只广告可执行项，三类graph是否明确keep-separate，current_plan接通Task artifact还是退役？ |
| Evolution mutation | `evolution-changelog-rollback-authority`, `evolution-skill-promotion-audit`, `evolution-doc-namespace` | Skill mutation是trusted-host primitive或必须携带ApprovalArtifact，rollback/cold tier/旧namespace承诺是什么？ |
| MCP reverse capability | `mcp-client-capability-advertisement` | 删除未实现roots/sampling/elicitation宣告，还是实现完整server-to-client request lifecycle？ |

## Confirmed Engineering Repair

| 边界 | 可独立修复的Finding |
| --- | --- |
| Context/Persistence | `transcript-projection-settlement`, `transcript-generation-runtime-identity`, `checkpoint-journal-binding`, `in-memory-audit-successful-drop`, `eval-trace-identity` |
| Task/Subagent | `task-patch-claim-race`, `task-subagent-attempt-link`, `subagent-factory-cancellation`, `subagent-factory-publication-race`, `background-task-wait` |
| Workflow/Scheduler/CommandCell | `workflow-checkpoint-claim-recovery`, `workflow-checkpoint-resurrection-race`, `workflow-parallel-failure-settlement`, `scheduler-cache-delivery`, `scheduler-task-id-uniqueness`, `scheduler-control-fire-race`, `command-cell-retention-lease-prune-race`, `command-cell-cancel-artifact-settlement` |
| Tool/Permission/Sandbox | `tool-read-cache-scope`, `tool-read-cache-inflight-invalidation-race`, `streaming-tool-validation`, `plan-mode-write-surface`, `readonly-tools-custom-registration-bypass`, `approval-authority`, `hook-protected-path`, `sandbox-minimum-isolation`, `sandbox-manager-stream-failure-typing`, `effect-cleanup-owner`, `k8s-sandbox-cleanup-settlement`, `tool-terminal-observation-divergence` |
| Extension | `skill-activation-authority`, `plugin-mcp-owner-isolation`, `plugin-generation-publication-authority`, `plugin-lifecycle-coordination`, `plugin-lifecycle-reconcile-overlap`, `lsp-runtime-state`, `extension-cleanup-settlement`, `extension-credential-debug-redaction`, `mcp-tool-permission-classification` |
| LLM | `nonstream-cancellation-parity`, `sse-eof-framing-acceptance`, `provider-stream-terminal-parity`, `tokenizer-calibration-feedback-convergence` |
| Protocol/SDK | `a2a-stream-cleanup`, `a2a-task-id-admission-authority`, `sdk-gap-generation-validation-parity` |
| Eval/Improve/Evolution | `eval-timeout-settlement`, `eval-workspace-generation-isolation`, `improve-iteration-config`, `improve-single-case-panic`, `background-review-detached-persistence-settlement`, `evolution-audit-atomicity`, `skill-candidate-reinforcement-audit-gap`, `pre-compaction-memory-trust-provenance` |

## Contract And Docs Repair

| 范围 | Finding |
| --- | --- |
| Workspace/README | `workspace-topology-doc-drift`, `public-feature-table-drift`, `readme-example-target-drift` |
| Observation/Hook/Tool | `tool-pipeline-example-drift` |
| Extension | `mcp-version-doc-drift` |
| Evolution | `evolution-doc-namespace` |

## 排序与依赖

第一优先级是会破坏状态或产生重复/泄漏effect且不依赖产品裁决的High Finding：Task claim、Subagent factory、Workflow checkpoint、Tool cache/validation、K8s cleanup、A2A admission/cleanup、Eval timeout/workspace。第二优先级是跨模块authority design：Turn commit、Plugin publication、provider capability与Evolution mutation。第三优先级是medium合同/文档修复，但与对应代码repair同批时必须同步完成。

每个独立repair开始前必须把一个可独立合并、验证、停止的outcome插入delivery map并创建单独Plan；可共享同一根因的Finding只有在不能独立正确交付时才进入同一outcome。

## 已知缺口

Decision Required尚未获得人的明确裁决；真实K8s/provider/A2A互操作、crash/loom/stress、GUI/product adapter和远端CI未执行。Plan04完成只表示高风险审计与修复路由可追踪。
