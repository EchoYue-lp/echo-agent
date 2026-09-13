---
schema_version: 1
id: evidence.improve-iteration-config-verification
kind: evidence
observed_at: 57066461ddbe8a32ce63f1b75dd40603530e786e
source_refs:
  - src/improve/eval_improvement.rs
supports: [behavior.eval-evolution, rule.quality-observation-boundary]
limitations:
  - 完整workspace合并门禁与远端CI尚未执行
  - 未执行真实LLM、report文件故障或并发workspace验收
---

# Improve iteration config 验证证据

## 支持的结论

旧实现上的真实run测试配置2，断言以actual 5、expected 2失败并exit 101。修复后的3个focused tests证明配置2产生2个iterations、配置0返回Some空iterations且factory调用数为0、disabled或empty cases继续返回None且不构造Agent。

## 来源与范围

Improve feature Clippy以`-D warnings`通过，`--no-default-features --features improve` check通过；`contracts/sdk`与`sdks/shared`相对基准零diff。Public setter、run签名、SDK route与文档用法不变。

## 已知缺口

本证据不覆盖真实LLM成本、HTML report持久化、workspace generation或其它Eval/Improve Finding。
