---
schema_version: 1
id: audit.guard-overlap-redaction-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: result_side_effect
freshness: examined
revision: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
finding_refs:
- finding.guard-overlap-redaction
challenges:
  owned-slice-and-counterexamples:
    revision: source:34d5f5574c9cf5a645970cd8b58367c39bce268630488b68d3ea54e996728e17
    source_refs:
    - echo-core/src/guard/content.rs
    - docs/en/18-guard-system.md
    - docs/zh/18-guard-system.md
    evidence_refs:
    - evidence.guard-overlap-redaction-repair
    - evidence.guard-overlap-redaction-verification
    - evidence.framework-guard-pivot-channel-focused-archive
---

# ContentGuard 重叠敏感区间重复替换损坏输出：独立局部复审

## 审查范围

独立framework_final_review在本次source摘要上检查新增真实切片、全部10修改/3新增、直接消费者、对应双语文档与ADR84/design/plan；两个focused manifest的owned源码及日志SHA一致。原28 frozen清单/tmp/framework-shared-validation-final-gates/frozen-implementation-docs.json保持一致。

## 已检查故障假设

原始偏移重叠、跨越、同起点不同长度及中文emoji包围；区间选择与最终文本无重复替换损伤。

## 实际实现路径与证据

ContentGuard 在原输入上合并重叠覆盖区间，选择最宽成员标记（同宽按既有detect顺序）；char_indices 单次输出，邻接不重叠区间分别保留。detect的原始byte offsets合同不变，未新增策略/API/SDK parser。

guard-red.log保留3项实际输出失败；guard-green.log覆盖完整content模块9 passed/0 failed/0 ignored。core-focused-clippy.log为production lib、guard feature、locked、warnings及unwrap/expect/panic/unreachable拒绝检查通过。owned源码格式与diff检查通过。

## 问题记录

局部source-level最终pass，Critical/Important/Minor均0；focused验证证据如上。修复仅关闭本Finding列出的故障，不关闭旧Guard方向、Channel generation/turn或其它开放义务。

## 残余风险

完整最后23尚无fresh receipt，不能给出完整最终工程pass；未证明公开pin、SDK三语言、CLI/官网与exact main。

## 未检查项

不扩大为框架全部风险视角已verified；独立后续完整验收只补工程receipt，不把源码局部复审重写成全量结论。
