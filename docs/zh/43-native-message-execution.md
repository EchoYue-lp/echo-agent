# 原生消息与执行结算

Framework 沿既有 ReAct 主循环接收 provider 原生消息。`think`、`final_answer` 控制工具及强制工具前分析提示已退役；模型的原生 thinking 配置、签名、加密块和必要历史回传继续保留。

Responses中的`content_part.added/done`只声明内容块及父项坐标，不声明父项类型。
解码器因此保留已知`output_item`类型；尚未收到类型时保持未知，不能默认猜成message而误拒
reasoning。ID仍立即登记，跨索引复用／同索引换ID、已知父项类型变化、完整响应与已观测
事实不一致等情况继续拒绝，不通过忽略身份校验兼容流事件。

## 公开返回形式与模型传输

`execute/chat` 返回 String，流式入口返回事件流；两者共用原 Managed owner、ReAct 和结束规则。
返回形式不决定供应商 HTTP 模式。`AgentConfig` 和 `ReactAgentBuilder` 的
`model_streaming(bool)` 默认 `true`，保持既有模型传输；只有显式 `false` 才调用同一
`LlmClient::chat`。完整非流式响应保留原生块、reasoning、工具来源和 usage 后进入同一
处理路径，事件流入口也可使用该模式。新执行仍以 canonical 消息为唯一默认文本来源，
不恢复 `Token/ThinkStart/ThinkEnd` 副本，不把普通正文映射成 Thought。

```rust,no_run
# use echo_agent::prelude::*;
# fn example() -> echo_agent::error::Result<()> {
let _agent = ReactAgentBuilder::new()
    .model_streaming(false)
    .build()?;
# Ok(())
# }
```

## 当前 typed producer 与便利入口

Core 已物理退役旧 AgentEvent Token/ThinkStart/ThinkEnd、旧 external-context setter
和 String source fallback；固定旧日志/恢复包保留原件，不赋予新执行兼容资格。
`execute_stream_message_with_invocation_context` 与
`chat_stream_message_with_invocation_context` 分别接纳完整 Message 与 Invocation。
source 保留 history、取消、资源、工具上界和工作目录，自己提供 Started、真实终态
和原 EOF。便利 String/raw-event 入口只委托对应当前 typed producer，不把 fresh
execute 当成多轮 chat，不从返回文字补造状态。

自定义 Agent 继续是合理公开能力，但应明确实现它支持的 typed producer；缺失入口
目前在 admission 前返回 `canonical typed execute invocation` 或
`canonical typed chat invocation`，不是成功，也不 fallback 到旧方法。
显示代码复用现有 `AgentOutputProjection`；runtime FinalAnswer 只确认批准的最终
结果，EOF 后补尚未展示的正文，不能双拼。当前示例见
[demo17_chat.rs](../../echo-agent-learning/examples/demo17_chat.rs) 与
[demo72_acp_agent_adapter.rs](../../echo-agent-learning/examples/demo72_acp_agent_adapter.rs)。

## 消息与工具的同一来源

`AgentEvent::AssistantMessage { block }` 保存 `block_id`、`request_id`、`sequence`、`phase`、`delta`、`complete`、`provider` 和原生 `reasoning`。阶段为 `reasoning`、`commentary`、`partial_answer`、`final_answer` 或 `unknown`；缺少阶段时保持 `unknown`，不按措辞或同响应是否有工具推断。

`AssistantMessageBlock`、`ToolCall` 与 `DeltaToolCall` 共用可选 `provider_source`。其中 `ProviderOutputSource` 保留协议、请求、原始 item 身份、`output_index`、`content_index`、content kind 与原始 phase。Responses 回放据此保持文字、reasoning 与工具的交错顺序，`refusal` 不改为 `output_text`。这些坐标只描述来源，不证明运行完成。

## 完整响应与唯一结束路径

有待执行工具时，先执行并回填匹配结果，再请求模型；同响应的 final 文字不能跳过工具。只有明确 commentary/partial 的响应，或 Anthropic 的 `pause_turn`，保存消息后继续同一执行。正常完成、有正文且无工具的未知阶段响应，可按 provider 协议成为最终候选。空响应、reasoning-only、截断和异常断流不构成成功。

最终候选共用结构化输出、已配置 Guard/Critic、Stop Hook、持久化与原执行 owner 结算。Stop 要求继续时保留同一 execution；整段校验未通过时不公开候选正文。成功公开顺序为批准的消息块、当前 runtime `FinalAnswer` 最终正文通知、最后单次 `ExecutionStatus(Settled)`。失败或取消保留真实原因后发布相应 runtime status；`ExecutionNotSettled` 是非终态观察。

`verifier_enabled` 只控制最终候选的 Critic 评审。应用默认启用 Critic 时，正常完整响应
中与工具调用同在、或位于明确协议 continue 边界的普通正文，若 effective phase 仍为
`Unknown`、未指定 JSON、且原 Output Guard 批准原文未转换，可进入原呈现及 approved
历史；phase 仍保 Unknown。它不是最终确认，工具状态也不能替代这段自然汇报。

显式 FinalAnswer 与工具共存时始终保留为未确认正文，包括早期 Unknown 增量由最后
空 metadata 明确为 Final 的情况；verifier 关闭也不提前公开。JSON Object/Schema 的
Unknown 中间原文、Guard 拒绝或转换后的原文继续 hold；异常终止及 partial-failure
路径不放开。明确 Commentary/PartialAnswer 与 Reasoning 保原发布规则，最终候选的
结构化校验、已配置 Critic、Stop、持久化及物理结算继续共用原路径。

消费者必须消费原流到运行时终态和原 EOF，不能在单块 `complete`、模型 `phase=final_answer` 或普通文字处关流。Agent 执行完成与 EKO 整个 TaskRun 目标完成分别由原有权威判断。

共享 TurnDriver 分别保存执行结果与投递结果。sink 关闭或失败后仍读取同一原流到 EOF，
保留原 producer 的 typed 错误及已观察的正文，尾部不会冒作已投递事件增加序号。由投递
失败诱发的取消通知不能盖掉原 sink 原因；真实 native 状态按原执行事实确认。
native unknown 需要匹配 execution 身份的运行时终态，正文投影或 foreign 状态均不能解除。

## 响应观察与不完整响应

`Message.provider_response` 和 `DeltaMessage.provider_response` 共用
`ProviderResponseMetadata`，保存真实协议、配置 provider、原 request ID、model、
stop_reason／stop_sequence、status／incomplete_details 与原生 usage。
字段缺失和显式 null 分别保留；未知停止字符串及 usage 的嵌套扩展不靠措辞归类。
这是同请求的完整观察快照，归一 token 计数仍归原预算，不把快照再次求和。

观察不证明正文批准、协议成功、源 EOF 或运行结算。原生 block／reasoning 回放仍走
原路径；模型历史 wire 剥除响应观察，不把 vendor status 当执行状态。

合法但未完成的响应由 `LlmError::IncompleteResponse { partial }` 失败返回。
直接 `LlmClient` 调用方可以检查 partial 及其观察；它不允许自动发布、重试或执行
其中的工具片段。Display 和 Agent 的有界错误不会输出整段 partial。
ReAct 失败时只把最后有效观察保存到原 Context 的 metadata-only assistant 记录；
这条记录无正文、工具、reasoning、批准内容或执行状态，下一模型请求整条排除。
完整 partial 不因此获得耐久正文或工具回放资格。

真实 HTTP／供应商错误、传输损坏、取消及未知物理结算保持原失败边界。
字段、优先级、冷恢复和消费者验收见[响应观察设计](../supreme/specs/provider-response-observation/design.md)。
具体协议与 SDK 的端到端资格以其当前源码和实际验证记录为准，基础 serde 通过不代表全链路完成。

## 持久化与恢复

原 `Message` 和 `ConversationStore` 保存互不替代的数据：

- `assistant_blocks`：provider 原件，供无损协议回放。
- `approved_assistant_blocks`：同一配置输出校验产生的呈现投影；显式空数组禁止回退到 raw 块。
- `execution_status`：持久候选坐标，关联执行、请求和末尾序号；候选本身不等同成功。
- `provider_response`：同一响应的诊断观察，不能替代上述任何批准或运行事实。

最终正文取受检的 `Message.content`。EKO 在原 ChatEventLog 保留实际交付的块、`FinalAnswer` 和最后 `Settled`，以原执行/请求/序号及完整 UTF-8 正文 SHA-256 核对恢复资格。缺少实际结算事实时保持未确认；诊断 Trace 的 Completed、provider phase 和块 complete 均不能替代。

旧日志、固定tag和已保全恢复包保持原件。依据用户开发期无兼容裁决，不再解析旧
think/final内置来源，不把旧断点或历史参数归一成新执行候选；不会为旧资料缺字段
猜身份、补结果、重新注册控制工具或重放已完成副作用。

当前新版本的cold/history/reload/Fork和Resume继续沿同一typed Message、严格checkpoint
配对/cursor校验、原Store reconcile/CAS及必要输入projector。当前runtime-only保存仍
保context-only lineage、transcript frontier、epoch和hydrated revision；缺失必要输入
或真实pending effect仍由原owner拒绝/结算。

原React准备owner在普通Store Load/恢复/Guard/准备的await期间保有同一admission、
execution mutex和资源束，成功后原对象移交同一producer；调用future被丢弃或typed
unknown不能提前释放资源。取消/绝对deadline不代替实际物理收尾。运行时FinalAnswer
事件和on_final_answer回调保留，仍由原校验与结算链触发。

## 工具输出的资源来源

原工具 publisher 将真实 `ToolResult.artifact` 与原文字保存在同条 canonical tool Message。
载体复用 `ContentPart::ResourceLink`，其 `meta.echo_tool_output_artifact` v1 记录原
call_id、tool_name、artifact 引用和可选显示范围。无 artifact 保持纯文本，失败工具结果
也不丢真实 artifact；普通路径或同名工具没有来源资格。

Parts 沿原 Context、live Steer 和 checkpoint 保存，不等待末尾事件重新拼接来源。
模型看到的资源文字仍计入原输入预算；该载体不表示工具成功或执行已结算。

## 验证边界

当前改造仍在实施与验收。源码、夹具回放、真实 Store 重开、EKO/SDK 消费、原生 GUI/IM 与正式 source/main CI 分别记录；单仓测试不能代替下游原生验收。架构依据见 [ADR 0101](../adr/0101-native-message-execution.md)。

## 公共请求与工具结果的传输边界

工具选择复用既有 `llm::ToolChoice`（Auto/None/Required/Function{name}），ChatRequest 与 ChatCompletionRequest 使用同一类型；Chat 是字符串策略或具名 function 对象，Responses/Anthropic 仅在各适配边界转换原生形状。不得把具名对象编码成 JSON 字符串，也不新增第二套工具选择模型。

`FunctionSpec.strict: Option<bool>` 独立于最终输出 `JsonSchemaSpec.strict`。None 在 Responses 保留既有 false 默认，Some(false)/Some(true) 必须原样兑现；其它协议仅按准确服务/端点/模型事实转换或显式拒绝。Text 使用默认文本，不能误判为不支持；JsonObject 与 JsonSchema 不互相伪装。

`Message.tool_result_is_error` 来自真实 ToolResult.success，普通执行返回业务错误也必须保 true；未知为 None。工具取消/错误不由该观察生成成功或任务终态，ExecutionNotSettled 继续原 owner 的观察与回收路径。

Framework 已按批准的 D1/D2 合同实现 `provider_response` 与
`LlmError::IncompleteResponse { partial }`，不再属于待裁决提案。它们复用现有
Message／Delta、ReAct／State 与有界失败投影；部分正文和未完成工具不取得批准、
重放或执行资格。准确字段、空值与错误首因见[响应观察设计](../supreme/specs/provider-response-observation/design.md)。

SDK 对既有已接受字段按其聚合值合同无损承接；这不自动把所有新 Rust 类型、构造器
或变体纳入独立跨语言承诺。SDK wire／Host／三语、精确 pin 与正式来源仍须各自验收，
有界 AgentFailure 不附完整 partial。当前两适配器 metadata-only EOF 的实际回放资格、
完整工程矩阵及真实 native 验收分别保留资格边界，不能由声明存在或根库编译通过替代。

testing 脚本的直接 Rust 构造使用 `StreamChunk::Delta(Box::new(delta))`。
装箱仅减少枚举内联大小，mock随后解出同一完整DeltaMessage；Message／Delta的公共观察
字段及任何序列化值不变，签名／opaque、工具与usage仍按原脚本传递。Rust source inventory
记录此构造签名后继，既有 SDK 字段值合同继续无损承接，不把Box视为新JSON形状。
