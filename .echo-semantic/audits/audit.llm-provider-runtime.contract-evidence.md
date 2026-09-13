---
schema_version: 1
id: audit.llm-provider-runtime.contract-evidence
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: contract_evidence
freshness: examined
revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
finding_refs: [finding.structured-output-main-path, finding.structured-output-schema-validation-contract, finding.provider-capability-authority, finding.model-fact-freshness-authority]
challenges:
  structured-output-main-path:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [src/agent/react/builder.rs, src/agent/snapshot.rs, src/agent/react/run/phases/think.rs, src/agent/react/extract.rs, docs/en/11-structured-output.md, echo-agent-learning/examples/demo15_structured_output.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  capability-owner:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-core/src/llm/mod.rs, echo-core/src/llm/capabilities.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/anthropic.rs, echo-state/src/compression/compressor/summary.rs, echo-state/src/compression/levels.rs]
    evidence_refs: [evidence.provider-protocol-quality]
  model-fact-freshness:
    revision: source:8b3972e1d2bc92f4ad59f511b6674eaaf243c1760f21973caf9e96558f71db90
    source_refs: [echo-core/src/llm/capabilities.rs, docs/en/38-factory-modes.md]
    evidence_refs: [evidence.provider-protocol-quality]
---

# LLM Structured Output 与 Capability 合同证据审计

## 审查范围

审查 Agent response_format、typed extraction/schema validation、LlmClient capabilities、LlmConfig/ModelProfile 与动态 model facts。

## 已检查故障假设

验证 builder schema 是否进入主 think，strict 是否在 framework 端校验，provider concrete client 是否准确声明 capability，以及内置模型事实是否有 provenance/freshness。

## 实际实现路径与证据

Builder 保存 response_format，但 RuntimeConfig 不携带，主 think 固定 None，execute_typed 只做执行后 serde parse。文档宣称全局强制，demo15 未真正配置且吞掉失败。JsonSchemaSpec strict 没有本地 JSON Schema validation。LlmClient 默认 OpenAI-compatible capabilities，concrete providers 未 override；Anthropic 明确拒绝 response_format，compressor 却按默认 true 发起再回退。LlmConfig/client 与 ModelProfile 是未连接的策略链；core 动态模型事实无来源版本/过期策略。

## 问题记录

确认 structured-output 与 provider capability Findings；新增 schema validation 与 model-fact freshness。response_format 作用阶段、capability owner 和 strict 保证需要 semantic-decide。

## 残余风险

直接把 response_format 应用于每轮 ReAct think 可能与 tool calling 冲突；内置事实不能被当作永久 provider 真理。

## 未检查项

未检查 live provider、第三方 LlmClient、全部 feature 组合或外部模型事实当前准确性。
