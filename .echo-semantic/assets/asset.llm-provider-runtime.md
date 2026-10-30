---
schema_version: 1
id: asset.llm-provider-runtime
kind: asset
title: LLM Provider 与请求协议边界
asset_type: protocol
status: active
risk: high
observed_at: source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636
boundary_refs:
- boundary.llm-provider-runtime
code_refs:
- echo-core/src/llm/mod.rs
- echo-core/src/llm/capabilities.rs
- echo-core/src/budget.rs
- echo-core/src/tokenizer.rs
- echo-integration/src/providers/config.rs
- echo-integration/src/providers/client.rs
- echo-integration/src/providers/openai.rs
- echo-integration/src/providers/anthropic.rs
- echo-integration/src/providers/responses.rs
consumer_refs:
- src/agent/react/run/phases/think.rs
- echo-state/src/compression/compressor/summary.rs
behavior_refs:
- behavior.llm-provider-execution
rule_refs:
- rule.provider-protocol-boundary
evidence_refs:
- evidence.provider-protocol-quality
- evidence.provider-stream-terminal-parity-repair
- evidence.provider-stream-terminal-parity-verification
- evidence.structured-output-main-path-verification
- evidence.nonstream-cancellation-parity-verification
- evidence.source8-provider-current-source-review
finding_refs:
- finding.structured-output-main-path
- finding.provider-capability-authority
- finding.nonstream-cancellation-parity
- finding.provider-stream-terminal-parity
candidate_refs: []
---

# 当前来源与有限父层接管

当前来源为原未改 verifier.source_digest(F) 实际返回的 `source:23e96814f826416af04c0771bde28c0bdf6eafb49fa05eda686b5a42e152b636`；不是 49d 工程 manifest 或自算 task。正式目录原24命令已全部自然0、源码无漂移；full05为4262外层通过／0失败／4忽略，93个Cargo运行目标，原78及旧default-path合79个具名用例各一次ok；独立learning doctest4通过、NoDefault和15单feature均通过。原official cargo-deny0.20.2四类自然0，原caller恰49组107身份、policy14通过；lock1442/baselinea5a6及风险合同不改。这些是当前工程范围，不能代签SDK accepted/Host/三语言、新pin、EKO/native、SSH/远端平台、性能、Git/mainCI或整个Discovery。

极大timeout已在原client边界checked_add并用既有ConfigError返回，保持None/零、覆盖、普通deadline与取消首因。原API before10为4通过6失败；d702同10通过，本次正式full05同10再次通过。三个既有overflow panic、first/idle typed配置缺口及far-future idle等待分别留原失败；u64最大毫秒在当前平台可表示的正控制通过，不任意夹值，不把内存bytes或混合14控制全称HTTP。

原父风险、完整code/consumer/behavior/rule/finding关系不缩减；以下准备段的needs_review/未取得等字样仅保原时点，当前可消费声明与上述具名修复按本节和机器observed_at。相邻Behavior/Rule、开放Finding、Discovery与真实未知不自动升级。

# 已冻结的r04准备记录原文

# LLM Provider 与请求协议边界

## 资产身份

Provider-neutral LlmClient/request、wire adapter、ModelProfile、budget/tokenizer/timeout的当前协议边界；不从模型别名或provider阶段推成功。

## 来源与消费者

ReactAgent think、compression/grader及外部消费者沿原client/model profile使用；来源优先级与请求能力由原resolver管理。原structured-output主路径、model precedence/freshness、non-stream取消及semantic-terminal各Finding的verification/rereview只按原范围接管，本轮具名增量见evidence.source8-provider-current-source-review。

## 生命周期

Build client→resolve profile/budget→request/stream→cancel/timeout/semantic terminal→usage calibration，保持同一原owner。Text/JSON object/JSON schema按真实协议能力分开；显式不支持不静默丢字段。非流取消覆盖send/headers/body/decode；共享SSE decoder在初始和后续buffer逐event交付前复核原cancel，防止yield后缓存finish/DONE越过取消。已发布终态及已观察原错误不被后来的cancel改写；usage在协议成功边界才发布。

## 候选关系

LlmConfig、client capabilities与ModelProfile是已明确分层的来源链；profile refresh、calibration和adapter保原权威，不增加产品permission或第二model loop。

## 未知与限制

历史observed_at为2f4da65cd5b83daa07d6c0f36d47bb50fd259c91；原“已形成Findings”不再概括后继实际verification/rereview。047a22旧候选曾依先前有限证据提出active，原件冻结。当前R15父层独审新增PE-R15-I1，故本后继明确保持needs_review，不得沿旧提案转active；observed_at:null仍待F正常caller合法来源绑定且不可直接采用。

PE-I1已从原source-only反例进入具名实际RED/GREEN；13条新增控制含共享decoder/Chat/OpenAI component的内存bytes组合，不能冒充真实HTTP/live-provider。R15另有原真实loopback控制及全量原票，场景分别列证据，不以总数替代覆盖。provider-specific tokenizer dispatch未实现，token估算/校准不是精确计费。真实供应商账号/互操作、外网抖动、平台/SDK/native及所有第三方LlmClient遵约未验；没有借本资产关闭其它状态或Discovery未知。

## R15 后继父审：极大 timeout 的当前阻断

独立报告provider-extension-parent-postfix-R15.md（SHA256 bc3caa0bd005ab771b2bd4d8f2514c5522eebb6a341337a3e8fd3177ba33bfce）结论Provider C0/I1/M0 block。PE-R15-I1：公开LlmTimeouts接受非零Duration::MAX/极大解码值，client.rs:696以Instant::now()+timeout在HTTP前建立deadline可panic；llm/mod.rs:155只排零而未核截止时间可表示性。这是当前公开输入/Result错误合同的实现反例，不能改称live-provider/native limitation或缩为正常timeout菜单后active。

该报告是source-only反例，未执行本项新增RED/GREEN；原PE缓存取消、structured/model/nonstream/terminal的已完成范围和原raw仍各保原scope，不被此次反例抹除。Provider deadline r02已获限定source pass，并进入统一after d702源码准备；实际NoRun/同断言RED/GREEN与稳定源工程资格在本稿准备时尚未取得；本增量不实现代码、不补新配置、不改risk或旧Finding/Audit，未占Cargo/source/final。Extension四类菜单有限pass与LSP/SDK历史承接独立保留，不由Provider blocker扩大为全部Extension失败。

## 本稿固定后继的资格分列

本稿初始取件基底为R15 `197cd657…`，当前四组修复固定组合为after manifest `d702396280f50ecc8138ccfbd53ae251061a6593451e91af90d337de2b98525f`，assembly `734021fa…`、执行计划 `1fba9fb4…`及其独立静态组合复核只证明准确准备与限定source范围。它们都是普通取件身份，不能填observed_at或冒task/source身份。

本稿准备时，原c01-before-01已进入编译的状态来自Root受控交接；本线不轮询/重启原operation，未取得可用于本后继绑定的实际NoRun自然0、实际test list或新反例RED/GREEN。函数清单是prepared selectors，编译/setup失败不能算行为RED；R15原24与fresh限定工程资格保原来源，不追给after d702。后续真实观察、稳定源工程与正常source/final绑定均交F sole，父status、风险、真实unknown、旧Eq/Finding/Discovery不由本材料改判。
