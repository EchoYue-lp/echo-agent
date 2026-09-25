---
schema_version: 1
id: finding.tokenizer-calibration-feedback-convergence
kind: finding
type: implementation_bug
status: resolved
severity: medium
primary_focus: time_lifecycle
focus: [state_authority, contract_evidence]
boundary_ref: boundary.llm-provider-runtime
behavior_refs: [behavior.llm-provider-execution]
rule_refs: [rule.provider-protocol-boundary]
evidence_refs: [evidence.provider-protocol-quality, evidence.tokenizer-calibration-feedback-repair, evidence.tokenizer-calibration-feedback-verification]
audit_refs: [audit.llm-provider-runtime.time-lifecycle, audit.tokenizer-calibration-feedback-rereview]
decision_refs: []
repair_evidence_refs: [evidence.tokenizer-calibration-feedback-repair]
verification_evidence_refs: [evidence.tokenizer-calibration-feedback-verification]
rereview_audit_refs: [audit.tokenizer-calibration-feedback-rereview]
discovered_at: f1e9027246760661144786e9e35615cd46d580c6
---

# Tokenizer calibration 生产反馈不收敛到真实比例

## 外部 Issue

GitHub Issue: https://github.com/EchoYue-lp/echo-agent/issues/100

## 问题

生产路径把已经乘过当前 factor 的 estimate 传给 calibrate，calibrate 又把 actual/adjusted 当新的绝对 factor 做 EMA；真实比例 2 会趋向 sqrt(2)。

## 触发条件与影响

长期 usage 回灌后 token budget 仍系统性低估/高估，且 message-only estimate 未含 tool schema，可能影响 compaction 与 context window 决策。

## 证据

`echo-core/src/tokenizer.rs`、`src/agent/react/run/phases/think.rs` 与 calibration tests/smoke 展示生产参数与测试不一致。

## 处理记录

候选修复以同一 ChatRequest 的未校准文本/Schema 估算与完整 prompt usage 配对；图像固定成本
不乘文本因子且含图像的 usage 不回灌。ContextManager 的准备和预压缩 Draft flush 使用同一
request overhead 预算规则，带大 format schema 的历史可先压缩再请求模型。原候选独立复审
三轮分别发现两项和一项 Important，修复后对预整合源码返回 PASS；整合快照的独立
增量复审也返回 PASS，定向测试、完整门禁与 17 项独立 feature 检查通过，Finding 置为
resolved。GitHub Issue #100 仍须远端 CI 与 main 交付后关闭。
