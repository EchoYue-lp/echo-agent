---
schema_version: 1
id: audit.data-pivot-value-fidelity-rereview
kind: audit
boundary_ref: boundary.tool-permission-sandbox
lens: contract_evidence
freshness: examined
revision: 4eb005c15b9c0036b73e409026c668c02f0a9575
finding_refs:
- finding.data-pivot-value-fidelity
challenges:
  owned-slice-and-counterexamples:
    revision: 4eb005c15b9c0036b73e409026c668c02f0a9575
    source_refs:
    - echo-tools/src/data.rs
    - docs/en/21-common-tools.md
    - docs/zh/21-common-tools.md
    evidence_refs:
    - evidence.data-pivot-value-fidelity-repair
    - evidence.data-pivot-value-fidelity-verification
    - evidence.framework-guard-pivot-channel-focused-archive
---


# Data Pivot 原生值与null组合及错误语义失真：独立局部复审

## 审查范围

独立framework_final_review在本次source摘要上检查新增真实切片、全部10修改/3新增、直接消费者、对应双语文档与ADR84/design/plan；两个focused manifest的owned源码及日志SHA一致。原28 frozen清单/tmp/framework-shared-validation-final-gates/frozen-implementation-docs.json保持一致。

## 已检查故障假设

native string/numeric/null分组及值，null index不丢失；含糊header失败关闭；不声明的aggregation不得用成功sum掩盖无效输入。

## 实际实现路径与证据

保留既有Polars grouping/aggregation/join权威；LiteralValue::from(AnyValue)保留原生值、null使用is_null、join使用nulls_equal。header与匹配值分离，冲突在join前返回InvalidParameter。aggregation只有缺省sum，声明外string及null/number/bool类型拒绝。未新增pivot引擎或平行validator。

pivot-red.log和pivot-adjacent-review-red.log分别保留真实3+3失败；pivot-invalid-runtime-red.log只是Tokio current-thread配置错误，明确不是行为反例。data-adjacent-review-green.log完整data模块14 passed/0 failed/0 ignored；tools-adjacent-review-clippy.log通过production约束，前序tools-focused-clippy保留。UTF-8输出、数值/null bucket与absent cell、null index、碰撞、aggregation类型/缺省均有actual-tool测试。

## 问题记录

局部source-level最终pass，Critical/Important/Minor均0；focused验证证据如上。修复仅关闭本Finding列出的故障，不关闭旧Guard方向、Channel generation/turn或其它开放义务。

## 残余风险

完整最后23尚无fresh receipt，不能给出完整最终工程pass；未证明公开pin、SDK三语言、CLI/官网与exact main。

## 未检查项

不扩大为框架全部风险视角已verified；独立后续完整验收只补工程receipt，不把源码局部复审重写成全量结论。


历史定位补充：上述34d5完整工程/独立审查绑定真实提交 4eb005c15b9c0036b73e409026c668c02f0a9575（唯一source摘要重算一致），后续依赖策略修复使当前source变化，不借此证明新版依赖通过。原命令、路径、时间、原结论与限制保持。
