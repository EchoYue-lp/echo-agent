# 原生消息与执行结算

Framework 沿既有 ReAct 主循环接收 provider 原生消息。`think`、`final_answer` 控制工具及强制工具前分析提示已退役；模型的原生 thinking 配置、签名、加密块和必要历史回传继续保留。

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

## 消息与工具的同一来源

`AgentEvent::AssistantMessage { block }` 保存 `block_id`、`request_id`、`sequence`、`phase`、`delta`、`complete`、`provider` 和原生 `reasoning`。阶段为 `reasoning`、`commentary`、`partial_answer`、`final_answer` 或 `unknown`；缺少阶段时保持 `unknown`，不按措辞或同响应是否有工具推断。

`AssistantMessageBlock`、`ToolCall` 与 `DeltaToolCall` 共用可选 `provider_source`。其中 `ProviderOutputSource` 保留协议、请求、原始 item 身份、`output_index`、`content_index`、content kind 与原始 phase。Responses 回放据此保持文字、reasoning 与工具的交错顺序，`refusal` 不改为 `output_text`。这些坐标只描述来源，不证明运行完成。

## 完整响应与唯一结束路径

有待执行工具时，先执行并回填匹配结果，再请求模型；同响应的 final 文字不能跳过工具。只有明确 commentary/partial 的响应，或 Anthropic 的 `pause_turn`，保存消息后继续同一执行。正常完成、有正文且无工具的未知阶段响应，可按 provider 协议成为最终候选。空响应、reasoning-only、截断和异常断流不构成成功。

最终候选共用结构化输出、已配置 Guard/Critic、Stop Hook、持久化与原执行 owner 结算。Stop 要求继续时保留同一 execution；整段校验未通过时不公开候选正文。成功公开顺序为批准的消息块、兼容 `FinalAnswer` 确认事件、最后单次 `ExecutionStatus(Settled)`。失败或取消保留真实原因后发布相应 runtime status；`ExecutionNotSettled` 是非终态观察。

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
失败诱发的 legacy 取消不能盖掉原 sink 原因；真实 native 状态按原执行事实确认。
native unknown 需要匹配 execution 身份的运行时终态，正文投影或 foreign 状态均不能解除。

## 持久化与恢复

原 `Message` 和 `ConversationStore` 保存三类互不替代的数据：

- `assistant_blocks`：provider 原件，供无损协议回放。
- `approved_assistant_blocks`：同一配置输出校验产生的呈现投影；显式空数组禁止回退到 raw 块。
- `execution_status`：持久候选坐标，关联执行、请求和末尾序号；候选本身不等同成功。

最终正文取受检的 `Message.content`。EKO 在原 ChatEventLog 保留实际交付的块、`FinalAnswer` 和最后 `Settled`，以原执行/请求/序号及完整 UTF-8 正文 SHA-256 核对恢复资格。缺少实际结算事实时保持未确认；诊断 Trace 的 Completed、provider phase 和块 complete 均不能替代。

旧日志保原件，不能准确区分的旧思考/工具前文字按历史执行说明读取。旧断点不重新注册控制工具、不重放已完成副作用、不补成功或重复 final。

已有 `PreModelContextProjector::recovered_tool_origins` 提供空默认来源入口；
`RecoveredToolOrigin/RecoveredToolKind` 只传运行期已核来源，不进入 Message、Checkpoint
或 SDK wire。应用按已核旧工厂与实际 custom 注册事实声明来源，框架只归一发往模型的
历史副本，保原 context、journal、cursor 和 CAS。名字、模型参数或 provider 坐标不能证明
旧 builtin 身份。已核来源的旧 pending 控制经纯读检查派生正常后继版本；原始 bytes 与
snapshot 先保全，再进入原 File CAS 和原 ReAct，不重新注册控制工具。旧 final 参数只是
历史候选说明；成功仍取当前模型输出及既有校验、结算事实。未知/custom、真实 pending
副作用或投递债务继续交原 owner，不补结果、不重放调用。默认 Store 不假定支持纯读恢复。

## 工具输出的资源来源

原工具 publisher 将真实 `ToolResult.artifact` 与原文字保存在同条 canonical tool Message。
载体复用 `ContentPart::ResourceLink`，其 `meta.echo_tool_output_artifact` v1 记录原
call_id、tool_name、artifact 引用和可选显示范围。无 artifact 保持纯文本，失败工具结果
也不丢真实 artifact；普通路径或同名工具没有来源资格。

Parts 沿原 Context、live Steer 和 checkpoint 保存，不等待末尾事件重新拼接来源。
模型看到的资源文字仍计入原输入预算；该载体不表示工具成功或执行已结算。

## 验证边界

当前改造仍在实施与验收。源码、夹具回放、真实 Store 重开、EKO/SDK 消费、原生 GUI/IM 与正式 source/main CI 分别记录；单仓测试不能代替下游原生验收。架构依据见 [ADR 0101](../adr/0101-native-message-execution.md)。
