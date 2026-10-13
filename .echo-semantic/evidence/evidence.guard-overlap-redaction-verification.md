---
schema_version: 1
id: evidence.guard-overlap-redaction-verification
kind: evidence
observed_at: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
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

# ContentGuard 重叠敏感区间重复替换损坏输出：验证

## 支持的结论

guard-red.log保留3项实际输出失败；guard-green.log覆盖完整content模块9 passed/0 failed/0 ignored。core-focused-clippy.log为production lib、guard feature、locked、warnings及unwrap/expect/panic/unreachable拒绝检查通过。owned源码格式与diff检查通过。

## 来源与范围

真实SDK Host曾把4513812345678111输出为 [REDACTED:CARD]E]111。旧实现按反序替换原偏移，重叠区间在已改变文本上失效；三项实际incorrect-output反例失败。日志未观察到UTF-8 panic，不能将潜在panic写为实际现象。

逐文件SHA、原始日志/receipt/manifest与before/after字节见 evidence.framework-guard-pivot-channel-focused-archive。独立framework_final_review已读历史24e3到当前全部10 modified+3 added及直接消费者，局部sourcepass/action0。原28 frozen文件保持与历史清单相同，不借用其旧完整23作为本增量证明。

## 已知缺口

新版完整工程门禁尚待执行；公开依赖、真实SDK各语言消费与精确main独立验收。Plan lifecycle保持ready，只有最终交付验收可推进。
