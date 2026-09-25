---
schema_version: 1
id: audit.model-facts-freshness-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: state_authority
freshness: examined
revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
finding_refs: [finding.model-fact-freshness-authority, finding.provider-capability-authority]
challenges:
  fresh-fact-precedence:
    revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
    source_refs: [echo-core/src/llm/capabilities.rs, src/agent/react/mod.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
  provider-protocol-cross-product:
    revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
    source_refs: [echo-core/src/llm/capabilities.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/anthropic.rs]
    evidence_refs: [evidence.model-facts-authority-verification]
  unknown-provider-isolation:
    revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
    source_refs: [echo-core/src/llm/capabilities.rs, echo-core/src/llm/mod.rs]
    evidence_refs: [evidence.model-facts-authority-verification]
  tokenizer-boundary:
    revision: source:e09ea140ba27724c9d3fa92c16a8139087ddfd0256c1f0d614f153e6086b54cd
    source_refs: [src/agent/snapshot.rs, docs/en/38-factory-modes.md, docs/adr/0047-model-facts-freshness-authority.md]
    evidence_refs: [evidence.model-facts-authority-repair]
---

# Model facts freshness 与 provider capability 独立复审

## 审查范围

Independent rereview 针对 `617547db` rebase 到 `origin/main@2490fd78` 后的
`72590bf4` 候选以及 fresh-wins follow-up。复审重点是 freshness precedence、无
`llm_client` 的 snapshot safe point、provider/protocol cross-product、unknown
provider isolation、legacy provider label compatibility、caller fact complement
和 tokenizer boundary。

## 实际复审结论

`ModelProfileResolver` 保持唯一合成权威。fresh provider/exact/caller facts
优先；旧 receipt 只补 fresh resolution 缺失的 source layer，caller records 继续
按字段保留互补值，空 fact 不会遮蔽 lower layer。无 live client 的 run snapshot
通过 `refresh_at` 重新检查 observed/expiry。

Anthropic adapter 的 protocol fact 在自定义 provider label 下仍发布真实能力；
内置 catalog 需要 provider label 与 wire protocol 匹配，unknown provider 不会按
模型名称启用 thinking、max-output 或 context facts。历史 OpenAI-compatible
provider labels 保持显式兼容映射。

## 验证范围

Focused core capability tests 25/25、provider integration tests 90/90、Agent
snapshot tests 22/22、fresh-wins merge tests（provider/exact/caller/empty）通过；
受影响 package Clippy 严格 deny 规则、fmt 和 diff-check 通过。完整 workspace
gate、17 feature matrix 和远端 CI 属于 delivery gate，不能由本 Audit 单独声称。

## Finding 处置

Framework authority、freshness、provider protocol boundary 和 merge precedence
均已覆盖；#68 与 framework portion of #77 可进入 resolved 状态。Structured output
schema enforcement、tokenizer dispatch、SDK facade 与 provider network refresh
仍由各自 Finding/后续边界追踪，不被本 Audit 隐式关闭。

## 残余风险

Runtime 仍使用 calibrated heuristic tokenizer；receipt 中的 tokenizer id 尚未
驱动 provider-specific tokenizer dispatch（后续 #100）。完整 delivery gate 尚未
在本 worktree 执行。

## 已检查故障假设

- 同 source layer 的旧 provider/exact/caller fact 覆盖 fresh v2。
- fresh caller 与 retained legacy caller 的互补字段丢失。
- empty fact 错误遮蔽 lower-precedence fact。
- provider label 与 wire protocol 交叉后激活错误 built-in catalog。
- 无 live `LlmClient` 时 retained resolution 不重新检查过期时间。

## 实际实现路径与证据

`merge_model_profile_resolution` 通过 `ModelProfileResolution::with_missing_source_layers_from`
重建 resolver；该方法在 source layer 缺失时补充 retained facts，在同 layer 已有 fresh
fact 时跳过旧记录，并把 caller facts 按字段裁剪后再按 resolver precedence 合成。协议
facts 与 provider metadata 保持分层，empty fact 不参与遮蔽。focused tests 覆盖 provider
v1→v2、exact v1→v2、caller 互补字段、empty exact、unknown provider/protocol isolation
和无 client snapshot refresh。

## 问题记录

本次独立复审未发现新的 framework blocker；上述故障假设均由 focused regression
或 source-level authority 检查覆盖。

## 未检查项

未执行完整 workspace gate、17 项 feature matrix 或远端 CI；SDK continuity errors
仍由 SDK/semantic governance 边界独立追踪。
