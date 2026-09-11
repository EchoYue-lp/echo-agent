---
schema_version: 1
id: baseline.repository
kind: baseline
source_snapshot:
  base_revision: 07f860ac168df500423fd93e16581b57603888de
  content_digest: a298b808735ab2ddd9f004d60954a526e2f7673da044b942cb08c1f3228d31ca
inventory_closure: open
behavior_model_closure: open
map_refs:
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
  - { path: Cargo.toml, status: supporting }
  - { path: LICENSE, status: supporting }
  - { path: README.md, status: supporting }
  - { path: README.zh.md, status: supporting }
  - { path: benches, status: supporting }
  - { path: contracts, status: in_scope }
  - { path: deny.toml, status: supporting }
  - { path: docs, status: supporting }
  - { path: echo-agent-learning, status: supporting }
  - { path: echo-core, status: supporting }
  - { path: echo-execution, status: supporting }
  - { path: echo-integration, status: supporting }
  - { path: echo-macros, status: supporting }
  - { path: echo-orchestration, status: in_scope }
  - { path: echo-sdk-host, status: in_scope }
  - { path: echo-sdk-protocol, status: in_scope }
  - { path: echo-state, status: supporting }
  - { path: echo-tools, status: supporting }
  - { path: mcp.json.example, status: supporting }
  - { path: rust-toolchain.toml, status: supporting }
  - { path: scripts, status: supporting }
  - { path: sdks, status: in_scope }
  - { path: src, status: in_scope }
  - { path: tests, status: supporting }
boundaries:
  - id: boundary.sdk-facade-parity
    map_ref: map.sdk-facade-parity
    risk: high
coverage: []
---

# echo-agent 语义基线

## 源码快照

基线绑定当前 Plan 8 工作树；长期材料自身不参与源码摘要。

## 仓库区域

全部当前 Git 路径已分类。SDK合同、Host、语言Client和直接框架适配器处于本次范围；其它框架实现与文档作为支持证据。

## 能力图与边界

当前只建立 `boundary.sdk-facade-parity`，覆盖根 facade到ACP与`_echo_agent/*`的适配边界。

## 覆盖网格

首次基线未宣称全仓风险视角闭合；覆盖网格将在后续定向发现中补齐。

## 未知与缺口

除SDK facade外的框架行为尚未建模；SDK facade的source operation、typed consumer trait和stream teardown已有focused证据，三语言可执行 route baseline 已闭合，intrinsic 行为仍待补齐。

## 闭合结论

路径库存与行为模型保持开放。本基线只允许对当前高风险变更建立可追踪依据，不代表仓库没有其它缺陷。
