---
schema_version: 1
id: baseline.repository
kind: baseline
source_snapshot:
  base_revision: a8a4d945f34ad2cfef92b149f205016c1c721b5e
  content_digest: fe131df2148d81ee04fef5804e38cf59a2e231e8e28d5e71b89323a5c2fa98b0
inventory_closure: closed
behavior_model_closure: closed
map_refs:
  - map.workspace-architecture
  - map.agent-session-turn
  - map.context-memory
  - map.task-subagent-workflow
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
  - map.extension-lifecycle
  - map.llm-provider-runtime
  - map.protocol-surfaces
  - map.eval-evolution
  - map.sdk-facade-parity
regions:
  - { path: .agents, status: supporting }
  - { path: .cargo, status: supporting }
  - { path: .example.env, status: supporting }
  - { path: .github, status: supporting }
  - { path: .gitignore, status: supporting }
  - { path: AGENTS.md, status: supporting }
  - { path: AUDIT_REPORT.md, status: supporting }
  - { path: CHANGELOG.md, status: supporting }
  - { path: CONTRIBUTING.md, status: supporting }
  - { path: Cargo.lock, status: supporting }
  - { path: Cargo.toml, status: in_scope }
  - { path: LICENSE, status: supporting }
  - { path: README.md, status: supporting }
  - { path: README.zh.md, status: supporting }
  - { path: benches, status: supporting }
  - { path: deny.toml, status: supporting }
  - { path: docs, status: supporting }
  - { path: echo-agent-learning, status: supporting }
  - { path: echo-core, status: in_scope }
  - { path: echo-execution, status: in_scope }
  - { path: echo-integration, status: in_scope }
  - { path: echo-macros, status: in_scope }
  - { path: echo-orchestration, status: in_scope }
  - { path: echo-state, status: in_scope }
  - { path: echo-tools, status: in_scope }
  - { path: mcp.json.example, status: supporting }
  - { path: rust-toolchain.toml, status: supporting }
  - { path: scripts, status: supporting }
  - { path: src, status: in_scope }
  - { path: tests, status: supporting }
boundaries:
  - { id: boundary.workspace-architecture, map_ref: map.workspace-architecture, risk: high }
  - { id: boundary.agent-session-turn, map_ref: map.agent-session-turn, risk: high }
  - { id: boundary.context-memory, map_ref: map.context-memory, risk: high }
  - { id: boundary.task-subagent-workflow, map_ref: map.task-subagent-workflow, risk: high }
  - { id: boundary.observation-persistence-delivery, map_ref: map.observation-persistence-delivery, risk: high }
  - { id: boundary.tool-permission-sandbox, map_ref: map.tool-permission-sandbox, risk: high }
  - { id: boundary.extension-lifecycle, map_ref: map.extension-lifecycle, risk: high }
  - { id: boundary.llm-provider-runtime, map_ref: map.llm-provider-runtime, risk: high }
  - { id: boundary.protocol-surfaces, map_ref: map.protocol-surfaces, risk: high }
  - { id: boundary.eval-evolution, map_ref: map.eval-evolution, risk: high }
  - id: boundary.sdk-facade-parity
    map_ref: map.sdk-facade-parity
    risk: high
coverage:
  - { region: Cargo.toml, lens: trigger_input, status: covered, refs: &cargo_refs [map.workspace-architecture] }
  - { region: Cargo.toml, lens: result_side_effect, status: covered, refs: *cargo_refs }
  - { region: Cargo.toml, lens: state_authority, status: covered, refs: *cargo_refs }
  - { region: Cargo.toml, lens: data_durability, status: covered, refs: *cargo_refs }
  - { region: Cargo.toml, lens: time_lifecycle, status: covered, refs: *cargo_refs }
  - { region: Cargo.toml, lens: failure_concurrency, status: covered, refs: *cargo_refs }
  - { region: Cargo.toml, lens: permission_external, status: covered, refs: *cargo_refs }
  - { region: Cargo.toml, lens: contract_evidence, status: covered, refs: *cargo_refs }
  - { region: echo-core, lens: trigger_input, status: covered, refs: &core_refs [map.workspace-architecture, map.agent-session-turn, map.context-memory, map.observation-persistence-delivery, map.tool-permission-sandbox, map.extension-lifecycle, map.llm-provider-runtime] }
  - { region: echo-core, lens: result_side_effect, status: covered, refs: *core_refs }
  - { region: echo-core, lens: state_authority, status: covered, refs: *core_refs }
  - { region: echo-core, lens: data_durability, status: covered, refs: *core_refs }
  - { region: echo-core, lens: time_lifecycle, status: covered, refs: *core_refs }
  - { region: echo-core, lens: failure_concurrency, status: covered, refs: *core_refs }
  - { region: echo-core, lens: permission_external, status: covered, refs: *core_refs }
  - { region: echo-core, lens: contract_evidence, status: covered, refs: *core_refs }
  - { region: echo-execution, lens: trigger_input, status: covered, refs: &execution_refs [map.tool-permission-sandbox, map.extension-lifecycle] }
  - { region: echo-execution, lens: result_side_effect, status: covered, refs: *execution_refs }
  - { region: echo-execution, lens: state_authority, status: covered, refs: *execution_refs }
  - { region: echo-execution, lens: data_durability, status: covered, refs: *execution_refs }
  - { region: echo-execution, lens: time_lifecycle, status: covered, refs: *execution_refs }
  - { region: echo-execution, lens: failure_concurrency, status: covered, refs: *execution_refs }
  - { region: echo-execution, lens: permission_external, status: covered, refs: *execution_refs }
  - { region: echo-execution, lens: contract_evidence, status: covered, refs: *execution_refs }
  - { region: echo-integration, lens: trigger_input, status: covered, refs: &integration_refs [map.extension-lifecycle, map.llm-provider-runtime, map.protocol-surfaces] }
  - { region: echo-integration, lens: result_side_effect, status: covered, refs: *integration_refs }
  - { region: echo-integration, lens: state_authority, status: covered, refs: *integration_refs }
  - { region: echo-integration, lens: data_durability, status: covered, refs: *integration_refs }
  - { region: echo-integration, lens: time_lifecycle, status: covered, refs: *integration_refs }
  - { region: echo-integration, lens: failure_concurrency, status: covered, refs: *integration_refs }
  - { region: echo-integration, lens: permission_external, status: covered, refs: *integration_refs }
  - { region: echo-integration, lens: contract_evidence, status: covered, refs: *integration_refs }
  - { region: echo-macros, lens: trigger_input, status: covered, refs: &macro_refs [map.workspace-architecture, map.tool-permission-sandbox, map.extension-lifecycle] }
  - { region: echo-macros, lens: result_side_effect, status: covered, refs: *macro_refs }
  - { region: echo-macros, lens: state_authority, status: covered, refs: *macro_refs }
  - { region: echo-macros, lens: data_durability, status: covered, refs: *macro_refs }
  - { region: echo-macros, lens: time_lifecycle, status: covered, refs: *macro_refs }
  - { region: echo-macros, lens: failure_concurrency, status: covered, refs: *macro_refs }
  - { region: echo-macros, lens: permission_external, status: covered, refs: *macro_refs }
  - { region: echo-macros, lens: contract_evidence, status: covered, refs: *macro_refs }
  - { region: echo-orchestration, lens: trigger_input, status: covered, refs: &orchestration_refs [map.agent-session-turn, map.task-subagent-workflow, map.observation-persistence-delivery, map.tool-permission-sandbox] }
  - { region: echo-orchestration, lens: result_side_effect, status: covered, refs: *orchestration_refs }
  - { region: echo-orchestration, lens: state_authority, status: covered, refs: *orchestration_refs }
  - { region: echo-orchestration, lens: data_durability, status: covered, refs: *orchestration_refs }
  - { region: echo-orchestration, lens: time_lifecycle, status: covered, refs: *orchestration_refs }
  - { region: echo-orchestration, lens: failure_concurrency, status: covered, refs: *orchestration_refs }
  - { region: echo-orchestration, lens: permission_external, status: covered, refs: *orchestration_refs }
  - { region: echo-orchestration, lens: contract_evidence, status: covered, refs: *orchestration_refs }
  - { region: echo-state, lens: trigger_input, status: covered, refs: &state_refs [map.context-memory, map.observation-persistence-delivery, map.eval-evolution] }
  - { region: echo-state, lens: result_side_effect, status: covered, refs: *state_refs }
  - { region: echo-state, lens: state_authority, status: covered, refs: *state_refs }
  - { region: echo-state, lens: data_durability, status: covered, refs: *state_refs }
  - { region: echo-state, lens: time_lifecycle, status: covered, refs: *state_refs }
  - { region: echo-state, lens: failure_concurrency, status: covered, refs: *state_refs }
  - { region: echo-state, lens: permission_external, status: covered, refs: *state_refs }
  - { region: echo-state, lens: contract_evidence, status: covered, refs: *state_refs }
  - { region: echo-tools, lens: trigger_input, status: covered, refs: &tool_refs [map.tool-permission-sandbox] }
  - { region: echo-tools, lens: result_side_effect, status: covered, refs: *tool_refs }
  - { region: echo-tools, lens: state_authority, status: covered, refs: *tool_refs }
  - { region: echo-tools, lens: data_durability, status: covered, refs: *tool_refs }
  - { region: echo-tools, lens: time_lifecycle, status: covered, refs: *tool_refs }
  - { region: echo-tools, lens: failure_concurrency, status: covered, refs: *tool_refs }
  - { region: echo-tools, lens: permission_external, status: covered, refs: *tool_refs }
  - { region: echo-tools, lens: contract_evidence, status: covered, refs: *tool_refs }
  - { region: src, lens: trigger_input, status: covered, refs: &root_refs [map.agent-session-turn, map.context-memory, map.task-subagent-workflow, map.observation-persistence-delivery, map.tool-permission-sandbox, map.extension-lifecycle, map.llm-provider-runtime, map.protocol-surfaces, map.eval-evolution] }
  - { region: src, lens: result_side_effect, status: covered, refs: *root_refs }
  - { region: src, lens: state_authority, status: covered, refs: *root_refs }
  - { region: src, lens: data_durability, status: covered, refs: *root_refs }
  - { region: src, lens: time_lifecycle, status: covered, refs: *root_refs }
  - { region: src, lens: failure_concurrency, status: covered, refs: *root_refs }
  - { region: src, lens: permission_external, status: covered, refs: *root_refs }
  - { region: src, lens: contract_evidence, status: covered, refs: *root_refs }
---

# echo-agent 全 Workspace 语义基线

## 源码快照

基线以远端 `main@bd17c730` 为可恢复祖先，绑定当前非语义源码摘要；任务分支的 merge commit 不作为目标主线祖先。`.echo-semantic` 自身不参与摘要。后续业务、合同、测试或正式文档变化必须通过 semantic-diff 刷新。

## 仓库区域

9 个 framework production 顶层区域进入 `in_scope`；tests、examples、docs、CI、scripts、
learning 和 repository metadata 是 supporting consumers。已迁出的 SDK 产品由独立仓库
承接，不再属于当前 framework workspace 区域。

## 能力图与边界

十张顶层 Capability Map 覆盖架构、Agent/Turn、Context、Task/Subagent/Workflow、Observation/Persistence、Tool/Permission、Extensions、LLM、Protocols 与 Eval/Evolution；现有 SDK facade map 作为 protocol 子图保留。

## 覆盖网格

每个 in-scope 区域的八个风险视角都有对应 map；`mapped` 包括正常路径和已记录 Finding，证据不足或需裁决的场景保留 `needs_review`/`unresolved`。

## 未知与缺口

当前 open Findings 与 Discovery unknown 是后续 high-risk audit frontier。闭合不表示缺陷已解决，也不表示所有动态 consumer 或外部环境已经运行验证。

## 闭合结论

路径库存和行为模型在当前快照上闭合：所有能力、已知问题和未知区均有可追踪处置。后续工作按 Finding 与风险边界推进，不按 SDK identity 数量推进。
