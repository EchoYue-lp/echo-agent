---
schema_version: 1
id: evidence.plugin-lifecycle-reconcile-repair
kind: evidence
observed_at: source:370d0d64574bf270a787a39a524b5e4e14066b5a87f253fe8ba5102d8aa74dd7
source_refs:
  - echo-core/src/plugin/lifecycle.rs
  - docs/adr/0060-plugin-lifecycle-reconcile-settlement.md
  - docs/en/32-plugin-system.md
  - docs/zh/32-plugin-system.md
supports: [behavior.extension-publication, rule.extension-generation-authority]
limitations:
  - 不协调PluginRegistry持久enabled状态与PluginIntegrator wiring
  - 不修复旧PreparedPluginSet重新apply和跨owner MCP撤销
  - 不以Drop或期望enabled集合变化替代callback cleanup结算
---

# Plugin callback reconcile 撤销失败阻断修复证据

## 支持的结论

原`PluginLifecycleManager::reconcile`在旧项`deactivate`失败后继续调用
`activate_in_set`；旧项仍为`active=true`且保留`cleanup_required`，新项则可能开始外部
effect。拆分的`deactivate_not_in` / `activate_enabled`和直接`activate`同样可绕过撤销结果。

现由原有Manager内的`cleanup_required`与独立`shutdown_required`统一阻断所有新增callback
激活。撤销失败仍持有旧registration，之后`deactivate`成功只结清撤销阶段债务；
`unregister`的shutdown失败持续阻断激活，直到shutdown重试成功。`init`失败虽然未设置
initialized，也须执行shutdown，防止部分获得资源；它不进入deactivate阶段，因为
activate从未开始。激活失败的
潜在部分effect同样由`unregister`完成清理。没有自身债务的已活跃callback保持幂等；
相同enabled集合重复reconcile不会重复激活。

## 来源与范围

框架`echo-core`原有callback Manager仍是唯一生命周期owner；未新增状态存储、generation
authority或EKO策略。ADR 0012负责不可变preparation、ADR 0045 DU-71禁止generation
混合，ADR 0060说明callback范围内的失败关闭与重试取舍。

## 已知缺口

#72 active generation、#73 跨Registry/wiring/callback协调及#75 MCP owner isolation
保持独立Finding。本修复证明Manager不在旧callback撤销失败后发布新callback，不证明
组件wiring与callbacks的原子热重载。
