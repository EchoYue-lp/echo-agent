---
schema_version: 1
id: evidence.guard-overlap-redaction-repair
kind: evidence
observed_at: 4eb005c15b9c0036b73e409026c668c02f0a9575
source_refs:
- echo-core/src/guard/content.rs
- docs/en/18-guard-system.md
- docs/zh/18-guard-system.md
supports:
- behavior.effect-permission-execution
- rule.permission-effect-order
limitations:
- Exact local source slice and focused tests only; fresh complete23, published pin, SDK/CLI/website and exact
  main require separate receipts.
- Independent incremental source review pass has zero actionable items; it does not certify every framework
  lens.
---


# ContentGuard 重叠敏感区间重复替换损坏输出：修复

## 支持的结论

ContentGuard 在原输入上合并重叠覆盖区间，选择最宽成员标记（同宽按既有detect顺序）；char_indices 单次输出，邻接不重叠区间分别保留。detect的原始byte offsets合同不变，未新增策略/API/SDK parser。

## 来源与范围

真实SDK Host曾把4513812345678111输出为 [REDACTED:CARD]E]111。旧实现按反序替换原偏移，重叠区间在已改变文本上失效；三项实际incorrect-output反例失败。日志未观察到UTF-8 panic，不能将潜在panic写为实际现象。

逐文件SHA、原始日志/receipt/manifest与before/after字节见 evidence.framework-guard-pivot-channel-focused-archive。独立framework_final_review已读历史24e3到当前全部10 modified+3 added及直接消费者，局部sourcepass/action0。原28 frozen文件保持与历史清单相同，不借用其旧完整23作为本增量证明。

## 已知缺口

新版完整工程门禁尚待执行；公开依赖、真实SDK各语言消费与精确main独立验收。Plan lifecycle保持ready，只有最终交付验收可推进。


历史定位补充：上述34d5完整工程/独立审查绑定真实提交 4eb005c15b9c0036b73e409026c668c02f0a9575（唯一source摘要重算一致），后续依赖策略修复使当前source变化，不借此证明新版依赖通过。原命令、路径、时间、原结论与限制保持。
