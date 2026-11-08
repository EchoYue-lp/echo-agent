---
schema_version: 1
id: evidence.implicit-helper-temperature
kind: evidence
title: 自动辅助请求省略隐式温度的定向回归
observed_at: source:bf9d087202711517ee3fdfcda0898d2d3a045c4bbe59568c103de7f8f9fb853f
source_refs:
- echo-core/src/llm/mod.rs
- echo-integration/src/providers/responses.rs
- src/agent/critic/llm_critic.rs
- src/agent/react/extract.rs
- src/agent/react/run/context.rs
- src/agent/react/run/phases/compact.rs
- src/evolution/background_review.rs
- src/testing/mock_llm.rs
- echo-state/src/compression/levels.rs
- echo-state/src/compression/compressor/summary.rs
- docs/zh/11-structured-output.md
- docs/en/11-structured-output.md
supports:
- rule.provider-protocol-boundary
limitations:
- 仅本Framework定向Rust、合成回环和请求字段记录；未运行真实外部API或用户凭据、GUI、App连接测试或新安装包。
- OpenAI gpt-5.1与DeepSeek deepseek-flash是当前源码识别的fixture模型名，不声称用户现场exactmodel或最新外部支持全集。
- 服务端access_rule/403与温度构造错误独立，本回归不证明该403已解决。
- 原合并矩阵、独立SDK/pin、平台/mainCI、R2候选与native资格不从本focused推定。
---

# 自动辅助请求省略隐式温度

## 支持的结论

通用ChatRequest原默认temperature=None。SimpleChatOptions默认曾主动提供Some(0.7)，
使原LlmClient.chat_simple路径在Responses默认推理配置下被请求构造层拒绝，或在可构造时
向wire发送了调用方未选择的采样字段。修复统一为None，max_tokens=2048保持；
SimpleChatOptions.with_temperature、ChatRequest.temperature、Agent配置和macro显式入口保留。
原provider显式温度支持/拒绝、错误类型/全文、reasoning、metadata和原生ID规则均不改。

本次全Framework检索确认九个真实生产自动请求点：SimpleChatOptions默认、
LlmCritic.call_llm_once、ReactAgent.extract_json、pre_compaction_flush、BackgroundReviewer.run_review，
以及Adaptive L4摘要、SummaryCompressor结构化摘要、IncrementalSummaryCompressor首次结构化摘要
和增量更新。全部从原固定Some改None，既有自然语言fallback本来None不另改。
这不以EKO是否调用某压缩器为删除框架合理API的理由。

## 来源与范围

基底为本仓c62397061bbe7c22ed169d7277c3ed07c054df4d，原clean非main任务分支。
新增10个回归方法实际NoRun/list并逐exact执行：core1、root4、state2、Responses3。
原test-only生产值下得到9个真实行为失败、1个显式Some拒绝控制通过：
DeepSeek default reasoning返回原typed SerializationError及完整温度消息，未制造网络错；
OpenAI thinking=None未强加推理模式，真实owned POST后因JSON含温度触发wire断言。
其余记录Critic/抽取/压缩前flush/后台review/四摘要请求的真实字段，默认值均反证为Some。

只改九个生产值后，同一测试断言10项全通过。独审指出测试fixture先取server错误可能遮请求首因，
已在原server完整await/stop/join与5秒界不变下优先处理client错误；三个受影响Responses方法
重新NoRun/list并通过，既有explicit_thinking_temperature_and_budget_never_silently_drop也exact1通过。
按具名去重覆盖11个方法，不将重复回归叠计。所有最终相关结果0失败/0忽略，原各filtered保原。
该fixture小修是测试诊断顺序，不宣称另一个生产故障或外部RED→GREEN。

四相关crate的lib/tests/no-default定向Clippy -D warnings自然0，整仓cargo fmt write/check
均自然0、输出0字节且无源码格式漂移。锁1442与独立duplicate baseline a5a6原字节保持，
没有重解析锁或新增依赖/feature/风险接受。测试额外温度记录仅cfg(test)，all_temperatures是
crate内测试取件，state recorder也只cfg(test)；未新增生产公共类型或业务状态权威。

原caller/raw/自然receipt/run、actual list、9个before/after测试程序与原FP/.d及精profile原JSON
保存在本仓ignored `.echo-semantic/local/framework-implicit-temperature-removal-20261103-r01/`。
red-proof SHA `6dd605b1c7f6d6dc4bd661662c864b69974f0a59bf8fb6e090487b45c7b45fc9`；
green-results SHA `1fa5e80f85510891964c48fead35fbd8160e1f0b95320080bbe0fb9d6326eda3`；
final-integration-results SHA `8a831eaacabc6a4b514d888ce932e232f8ab09cdc2b77aa60fd16b0f1c8781d6`。
首次NoRun E0507/101属于新增测试guard移动boxed错误的类型问题，借用修正后取得NoRun0/W0；
本地label校验、OpenAI自定义assert文本读取规则和cfg(test)边界脚本误判均保原事件，
未冒行为RED或重复执行已完成方法。真正默认/helper RED与原始typed/wire原因分别保真。

## 已知缺口

公开函数缺省注释、中文结构化输出文档及原有en文档过时0.0段已同步事实，不新建英文副本。
learning demo10和demo25的显式0.7属于调用方opt-in，公开调用形状不变，无需改源码；
示例可执行链仍属于后继适用完整门禁。本线不直接写官网sole源码，只提供其zh/en旧0.0段的
精base-SHA文段补丁，由Root/Release owner采用；不能把这个候选说成网站已发布。

当前App c548 retained继续由用户手验，本线没有退出/重开/自动界面/RPC或用户配置/密钥操作。
此Framework源码修复不代表运行App已经更新，也不自动解除服务端403访问规则。
当前源码的正常Source/explicit Final按其真实原件绑定；全局closure仍按实际范围，不冒合并/mainCI。
