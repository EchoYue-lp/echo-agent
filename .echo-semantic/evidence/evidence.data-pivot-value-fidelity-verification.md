---
schema_version: 1
id: evidence.data-pivot-value-fidelity-verification
kind: evidence
observed_at: 4eb005c15b9c0036b73e409026c668c02f0a9575
source_refs:
- echo-tools/src/data.rs
- docs/en/21-common-tools.md
- docs/zh/21-common-tools.md
supports:
- behavior.effect-permission-execution
- rule.permission-effect-order
limitations:
- Exact local source slice and focused tests only; fresh complete23, published pin, SDK/CLI/website and exact
  main require separate receipts.
- Independent incremental source review pass has zero actionable items; it does not certify every framework
  lens.
---


# Data Pivot 原生值与null组合及错误语义失真：验证

## 支持的结论

pivot-red.log和pivot-adjacent-review-red.log分别保留真实3+3失败；pivot-invalid-runtime-red.log只是Tokio current-thread配置错误，明确不是行为反例。data-adjacent-review-green.log完整data模块14 passed/0 failed/0 ignored；tools-adjacent-review-clippy.log通过production约束，前序tools-focused-clippy保留。UTF-8输出、数值/null bucket与absent cell、null index、碰撞、aggregation类型/缺省均有actual-tool测试。

## 来源与范围

旧AnyValue Display字符串被当filter literal：字符串带引号、数值与string比较失败、null bucket丢失。独立复审又发现null index join丢失、null/string label冲突和未知aggregation静默sum。原3及追加3实际输出/错误断言均失败。

逐文件SHA、原始日志/receipt/manifest与before/after字节见 evidence.framework-guard-pivot-channel-focused-archive。独立framework_final_review已读历史24e3到当前全部10 modified+3 added及直接消费者，局部sourcepass/action0。原28 frozen文件保持与历史清单相同，不借用其旧完整23作为本增量证明。

## 已知缺口

新版完整工程门禁尚待执行；公开依赖、真实SDK各语言消费与精确main独立验收。Plan lifecycle保持ready，只有最终交付验收可推进。


历史定位补充：上述34d5完整工程/独立审查绑定真实提交 4eb005c15b9c0036b73e409026c668c02f0a9575（唯一source摘要重算一致），后续依赖策略修复使当前source变化，不借此证明新版依赖通过。原命令、路径、时间、原结论与限制保持。
