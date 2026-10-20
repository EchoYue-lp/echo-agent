---
schema_version: 1
id: audit.model-facts-freshness-rereview
kind: audit
boundary_ref: boundary.llm-provider-runtime
lens: state_authority
freshness: examined
revision: 382cb600b0c8c52d1f6866682c6553e0c3369409
finding_refs: [finding.model-fact-freshness-authority, finding.provider-capability-authority, finding.runtime-token-limit-explicit-publication, finding.runtime-model-client-publication]
challenges:
  configured-builder-client-pair-complete-local-verification:
    revision: ffe4c19acf6d33cd419f2237de3209b4e3015e80
    source_refs: [src/agent/react/builder.rs, src/agent/react/mod.rs, src/agent/react/tests.rs, docs/adr/0047-model-facts-freshness-authority.md, docs/en/38-factory-modes.md, docs/zh/38-factory-modes.md, echo-agent-learning/examples/demo26_provider_factory.rs]
    evidence_refs: [evidence.configured-client-construction]
  configured-builder-client-pair:
    revision: source:5f000f23fee488a6fecb1a972b155cd72ad206384c05f3802cdfec0d52c11872
    source_refs: [src/agent/react/builder.rs, src/agent/react/mod.rs, src/agent/react/tests.rs, docs/adr/0047-model-facts-freshness-authority.md, echo-agent-learning/examples/demo26_provider_factory.rs]
    evidence_refs: [evidence.configured-client-construction]
  configured-client-construction:
    revision: 382cb600b0c8c52d1f6866682c6553e0c3369409
    source_refs: [src/agent/react/mod.rs, src/agent/react/tests.rs, docs/adr/0047-model-facts-freshness-authority.md, docs/en/38-factory-modes.md, docs/zh/38-factory-modes.md]
    evidence_refs: [evidence.configured-client-construction]
  runtime-explicit-limit-publication:
    revision: f9906007599a9c9842474a65f4d8cb5a8a5228b8
    source_refs: [src/agent/config.rs, src/agent/react/capabilities.rs, src/agent/snapshot.rs, src/agent/react/run/phases/prepare.rs]
    evidence_refs: [evidence.compression-token-tail, evidence.model-facts-authority-verification]
  runtime-model-frozen-publication:
    revision: f9906007599a9c9842474a65f4d8cb5a8a5228b8
    source_refs: [src/agent/react/capabilities.rs, src/agent/react/mod.rs, src/agent/react/builder.rs, src/agent/react/tests.rs, src/channels.rs]
    evidence_refs: [evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
  fresh-fact-precedence:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
    source_refs: [echo-core/src/llm/capabilities.rs, src/agent/react/mod.rs, src/agent/react/tests.rs]
    evidence_refs: [evidence.model-facts-authority-repair, evidence.model-facts-authority-verification]
  provider-protocol-cross-product:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
    source_refs: [echo-core/src/llm/capabilities.rs, echo-integration/src/providers/config.rs, echo-integration/src/providers/anthropic.rs]
    evidence_refs: [evidence.model-facts-authority-verification]
  unknown-provider-isolation:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
    source_refs: [echo-core/src/llm/capabilities.rs, echo-core/src/llm/mod.rs]
    evidence_refs: [evidence.model-facts-authority-verification]
  tokenizer-boundary:
    revision: 07e4270380c40df3f412f99c0aecb6145410cb2a
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

## Runtime publication challenges (2026-10-19)

The two current challenges bind the framework candidate above. Historical
challenge revisions and old test totals remain unchanged and historical.
Independent final source rereview returned C0/I0/M0 after correcting one Important
conditional caller: the channels configured factory now performs its original
single build before Agent construction, resolves through the same preparation
owner and invokes the pure publisher. The isolated channels feature compile
passed. No public rebind API or channel product strategy was added.

The explicit-limit challenge retains the direct/prepared success marker and
busy/invalid/dropped non-publication policy, with actual compressor allowances
before and after real turns. The model challenge covers real HTTP A/B, typed
configuration refusal and rollback, no-op identity consistency, fresh sourced
precedence/expiry/complement, custom-client stale-config removal and prepared
profile resolution exclusively before commit. Matching existing prepared and
configuration-only behavior is preserved. Details, actual current command
receipts and bounded counts are in the existing verification Evidence.

Only these framework publication Findings close here. Broad Behavior statuses,
older provider/tokenizer frontiers, the SDK factory/getter adaptation, native task
Stop, full engineering/feature gates, cold-history retention and exact-main CI
are not promoted by this source review. The task-reader failure remains recorded;
the official full strict check must separately validate actual Git history.

## Configured-client constructor challenge

Only the new construction challenge binds source:1828e264adb8119b8b19c84ccc0e18b35067b6333f89c775e51856563290659e. Existing publication
challenges keep their actual f990 revisions and historical outcomes. The new
entry reuses one merge/publisher, preserves original Arc/config and implicit
initialization, rejects invalid pairs without secret-bearing diagnostics, and
leaves opaque replacement semantics intact. evidence.configured-client-construction records actual focused
consumption, including real HTTP and Rustdoc, and the separate fixture errors.
Independent implementation/rereview and per-file necessity are still required
before the normal milestone; full gates and new SDK/main remain separate.

## Builder pair challenge awaiting focused qualification

The new challenge alone binds the explicit configured Builder increment. The
Audit main revision/freshness and all historical challenges stay unchanged.
Check original Arc retention, one pre-Context profile read, real initial budgets,
shared registry/tools/store injection, invalid-pair rejection and secret-safe
diagnostics, old setter precedence and pair replacement, then actual same-Agent
HTTP A/B consumption. Current source inspection is not executed verification or
independent implementation review; neither is complete at this checkpoint.

## Builder complete local challenge rereview

The new named challenge binds actual public source ffe4c19acf6d33cd419f2237de3209b4e3015e80 /
source:5f000f23fee488a6fecb1a972b155cd72ad206384c05f3802cdfec0d52c11872. The earlier awaiting-focused section
records its prior static checkpoint. Main Audit revision/freshness, every old
challenge and broad Behavior/Rule statuses retain their original qualifications.

Independent implementation and exact-file necessity review passed C0/I0/M0;
focused original receipts and complete23 receipts also independently passed
C0/I0/M0 after checking original argv, all2,329 inputs, engineering1,764 inputs,
profile/source guards, raw completeness, test-harness boundaries and resource
limits. Bounded counterexamples cover loss of original Arc or rebuild config,
second profile query/client build, initial budget drift, dropped registry/tools/
store injection, old-setter precedence, stale config after opaque replacement,
invalid-pair diagnostics and actual HTTP A/B consumption.

Full05 has3,658pass/0fail/3 pre-existing external ignores/0filtered; isolated
features only compile. The separate attempt01 resource/guard failure remains
unknown as to Cargo natural exit and is not the CLI behavior before or a passed
run. The ignored guard repair is not causal proof for historical SDK Host exit.
This challenge closes only current local Builder source/engineering/rereview.
CLI original17 after-consumption, newmain/seven CI checks, native Hook/Stop,
final cold necessary refs and website mirrors remain separate; no global status
upgrade, new SDK contract or publication is implied.
