---
schema_version: 1
id: baseline.repository
kind: baseline
source_snapshot:
  base_revision: 95ec66c6083a8feb17a1c28de0d01e1a24ca8a28
  content_digest: 1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c
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
- path: .agents
  status: supporting
- path: .cargo
  status: supporting
- path: .example.env
  status: supporting
- path: .github
  status: supporting
- path: .gitignore
  status: supporting
- path: AGENTS.md
  status: supporting
- path: AUDIT_REPORT.md
  status: supporting
- path: CHANGELOG.md
  status: supporting
- path: CONTRIBUTING.md
  status: supporting
- path: Cargo.lock
  status: supporting
- path: Cargo.toml
  status: in_scope
- path: LICENSE
  status: supporting
- path: README.md
  status: supporting
- path: README.zh.md
  status: supporting
- path: benches
  status: supporting
- path: deny.toml
  status: supporting
- path: docs
  status: supporting
- path: echo-agent-learning
  status: supporting
- path: echo-core
  status: in_scope
- path: echo-execution
  status: in_scope
- path: echo-integration
  status: in_scope
- path: echo-macros
  status: in_scope
- path: echo-orchestration
  status: in_scope
- path: echo-state
  status: in_scope
- path: echo-tools
  status: in_scope
- path: mcp.json.example
  status: supporting
- path: rust-toolchain.toml
  status: supporting
- path: scripts
  status: supporting
- path: src
  status: in_scope
- path: tests
  status: supporting
boundaries:
- id: boundary.workspace-architecture
  map_ref: map.workspace-architecture
  risk: high
- id: boundary.agent-session-turn
  map_ref: map.agent-session-turn
  risk: high
- id: boundary.context-memory
  map_ref: map.context-memory
  risk: high
- id: boundary.task-subagent-workflow
  map_ref: map.task-subagent-workflow
  risk: high
- id: boundary.observation-persistence-delivery
  map_ref: map.observation-persistence-delivery
  risk: high
- id: boundary.tool-permission-sandbox
  map_ref: map.tool-permission-sandbox
  risk: high
- id: boundary.extension-lifecycle
  map_ref: map.extension-lifecycle
  risk: high
- id: boundary.llm-provider-runtime
  map_ref: map.llm-provider-runtime
  risk: high
- id: boundary.protocol-surfaces
  map_ref: map.protocol-surfaces
  risk: high
- id: boundary.eval-evolution
  map_ref: map.eval-evolution
  risk: high
- id: boundary.sdk-facade-parity
  map_ref: map.sdk-facade-parity
  risk: high
coverage:
- region: Cargo.toml
  lens: trigger_input
  status: covered
  refs: &id001
  - map.workspace-architecture
- region: Cargo.toml
  lens: result_side_effect
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: state_authority
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: data_durability
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: time_lifecycle
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: failure_concurrency
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: permission_external
  status: covered
  refs: *id001
- region: Cargo.toml
  lens: contract_evidence
  status: covered
  refs: *id001
- region: echo-core
  lens: trigger_input
  status: covered
  refs: &id002
  - map.workspace-architecture
  - map.agent-session-turn
  - map.context-memory
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
  - map.extension-lifecycle
  - map.llm-provider-runtime
- region: echo-core
  lens: result_side_effect
  status: covered
  refs: *id002
- region: echo-core
  lens: state_authority
  status: covered
  refs: *id002
- region: echo-core
  lens: data_durability
  status: covered
  refs: *id002
- region: echo-core
  lens: time_lifecycle
  status: covered
  refs: *id002
- region: echo-core
  lens: failure_concurrency
  status: covered
  refs: *id002
- region: echo-core
  lens: permission_external
  status: covered
  refs: *id002
- region: echo-core
  lens: contract_evidence
  status: covered
  refs: *id002
- region: echo-execution
  lens: trigger_input
  status: covered
  refs: &id003
  - map.tool-permission-sandbox
  - map.extension-lifecycle
- region: echo-execution
  lens: result_side_effect
  status: covered
  refs: *id003
- region: echo-execution
  lens: state_authority
  status: covered
  refs: *id003
- region: echo-execution
  lens: data_durability
  status: covered
  refs: *id003
- region: echo-execution
  lens: time_lifecycle
  status: covered
  refs: *id003
- region: echo-execution
  lens: failure_concurrency
  status: covered
  refs: *id003
- region: echo-execution
  lens: permission_external
  status: covered
  refs: *id003
- region: echo-execution
  lens: contract_evidence
  status: covered
  refs: *id003
- region: echo-integration
  lens: trigger_input
  status: covered
  refs: &id004
  - map.extension-lifecycle
  - map.llm-provider-runtime
  - map.protocol-surfaces
- region: echo-integration
  lens: result_side_effect
  status: covered
  refs: *id004
- region: echo-integration
  lens: state_authority
  status: covered
  refs: *id004
- region: echo-integration
  lens: data_durability
  status: covered
  refs: *id004
- region: echo-integration
  lens: time_lifecycle
  status: covered
  refs: *id004
- region: echo-integration
  lens: failure_concurrency
  status: covered
  refs: *id004
- region: echo-integration
  lens: permission_external
  status: covered
  refs: *id004
- region: echo-integration
  lens: contract_evidence
  status: covered
  refs: *id004
- region: echo-macros
  lens: trigger_input
  status: covered
  refs: &id005
  - map.workspace-architecture
  - map.tool-permission-sandbox
  - map.extension-lifecycle
- region: echo-macros
  lens: result_side_effect
  status: covered
  refs: *id005
- region: echo-macros
  lens: state_authority
  status: covered
  refs: *id005
- region: echo-macros
  lens: data_durability
  status: covered
  refs: *id005
- region: echo-macros
  lens: time_lifecycle
  status: covered
  refs: *id005
- region: echo-macros
  lens: failure_concurrency
  status: covered
  refs: *id005
- region: echo-macros
  lens: permission_external
  status: covered
  refs: *id005
- region: echo-macros
  lens: contract_evidence
  status: covered
  refs: *id005
- region: echo-orchestration
  lens: trigger_input
  status: covered
  refs: &id006
  - map.agent-session-turn
  - map.task-subagent-workflow
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
- region: echo-orchestration
  lens: result_side_effect
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: state_authority
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: data_durability
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: time_lifecycle
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: failure_concurrency
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: permission_external
  status: covered
  refs: *id006
- region: echo-orchestration
  lens: contract_evidence
  status: covered
  refs: *id006
- region: echo-state
  lens: trigger_input
  status: covered
  refs: &id007
  - map.context-memory
  - map.observation-persistence-delivery
  - map.eval-evolution
- region: echo-state
  lens: result_side_effect
  status: covered
  refs: *id007
- region: echo-state
  lens: state_authority
  status: covered
  refs: *id007
- region: echo-state
  lens: data_durability
  status: covered
  refs: *id007
- region: echo-state
  lens: time_lifecycle
  status: covered
  refs: *id007
- region: echo-state
  lens: failure_concurrency
  status: covered
  refs: *id007
- region: echo-state
  lens: permission_external
  status: covered
  refs: *id007
- region: echo-state
  lens: contract_evidence
  status: covered
  refs: *id007
- region: echo-tools
  lens: trigger_input
  status: covered
  refs: &id008
  - map.tool-permission-sandbox
- region: echo-tools
  lens: result_side_effect
  status: covered
  refs: *id008
- region: echo-tools
  lens: state_authority
  status: covered
  refs: *id008
- region: echo-tools
  lens: data_durability
  status: covered
  refs: *id008
- region: echo-tools
  lens: time_lifecycle
  status: covered
  refs: *id008
- region: echo-tools
  lens: failure_concurrency
  status: covered
  refs: *id008
- region: echo-tools
  lens: permission_external
  status: covered
  refs: *id008
- region: echo-tools
  lens: contract_evidence
  status: covered
  refs: *id008
- region: src
  lens: trigger_input
  status: covered
  refs: &id009
  - map.agent-session-turn
  - map.context-memory
  - map.task-subagent-workflow
  - map.observation-persistence-delivery
  - map.tool-permission-sandbox
  - map.extension-lifecycle
  - map.llm-provider-runtime
  - map.protocol-surfaces
  - map.eval-evolution
- region: src
  lens: result_side_effect
  status: covered
  refs: *id009
- region: src
  lens: state_authority
  status: covered
  refs: *id009
- region: src
  lens: data_durability
  status: covered
  refs: *id009
- region: src
  lens: time_lifecycle
  status: covered
  refs: *id009
- region: src
  lens: failure_concurrency
  status: covered
  refs: *id009
- region: src
  lens: permission_external
  status: covered
  refs: *id009
- region: src
  lens: contract_evidence
  status: covered
  refs: *id009
---






# echo-agent 全 Workspace 语义基线

## 源码快照

本候选基线以本地任务启动 `95ec66c6083a8feb17a1c28de0d01e1a24ca8a28` 为可恢复祖先，绑定当前非语义源码摘要；任务分支的 merge commit 不作为目标主线祖先。`.echo-semantic` 自身不参与摘要。后续业务、合同、测试或正式文档变化必须通过 semantic-diff 刷新。

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

当前 Task runtime/ACP lease 修复已映射并具有完整本地工程门禁。库存/模型的 closed 只表示场景有明确处置，不表示所有 Behavior verified；其它 needs_review/Open Finding、消费者、远端 main 验收仍保持各自证据边界。既有修复Finding的本地resolved与其历史工程/独立复审链保持；公开pin、消费者和main继续独立验收。


本轮新增 Guard 重叠区间、Pivot typed/null/error 与 Channel owned 操作已归原能力图，各自有修复、focused 验证和独立局部复审证据。当前模型观察绑定 source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17；历史 ecac/b0d162/8896 Evidence 保持可恢复 Git 和原限制。新版完整工程门禁、发布后的消费者与精确 main 仍独立待验，不以模型 closed 代替。


当前完整工程门禁补齐：evidence.framework-final-34d5-full-verification 绑定当前34d5源码、23exit0/warnings0、3471/0/3ignored/86targets/59examples；evidence.framework-final-34d5-gate-archive 保存实际47原文件。库存与模型closed含明确未知处置，依然不等于全部Behavior verified。SDK/remote main等独立验收保持。


当前候选最终独立实现复审 audit.framework-final-34d5-gate-rereview 已PASS/action0，完整源码+新23receipt归档闭合，允许进入用户授权的framework里程碑交付；交付尚须实际PR/CI/main证据。Planready与其它开放义务仍保持。


本次依赖策略增量：旧34d5门禁/复审已保留为真实4eb Git历史；新源码含6个Cargo文件和依赖修复原始证据，绑定source:1c515c09873968a279e6076cf6abf659923f1fa96137144423970bfcd0e1881c。依赖Finding与新增场景归原workspace map，focused459及审计red/green不替代新完整门禁/独立审查/远端dependency-policy CI。


本轮完整工程结果：依赖修复新23最终全部通过，第一次baselineancestor错误导致的真实test101及修复同命令attempt2通过均保留 evidence.framework-dependency-final-gate-archive。semantic写完后需针对baseline3再验；旧34d5 Gate/Audit只历史定位，新依赖源码局部review/工程绿不替代远端main或SDK消费者。
