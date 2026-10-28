# Provider 原生消息适配

`echo-integration` 解码供应商 wire，直接填写 Framework 的
`Message.assistant_blocks`／`DeltaMessage.assistant_blocks`。两者使用同一个
`AssistantMessageBlock`，不是 JSON sidecar，也不另建执行器或最终态权威。旧
`content`、`reasoning_content`、`reasoning_blocks`、`tool_calls` 保留为现有处理器和协议历史回传的投影；消费者不能因看到双投影重复发布消息。

## 三条真实协议路径

| 协议／适配器 | 当前消费者族 | 正文与阶段 | reasoning／工具 | 完整边界 |
| --- | --- | --- | --- | --- |
| Responses／`responses.rs` | OpenAI、明确配置 Responses 的兼容服务 | 每个 message/content 保 id、request、sequence；明确 phase 按字段转换，未知保持 Unknown | summary/content 独立块；encrypted_content 原样保留；工具保 call_id 和 output_index | 完整 `response.completed` 与非流式同一解析；未知／未完成 item、空、reasoning-only 均不能成功 |
| Anthropic／`anthropic.rs` | Anthropic、明确配置 Messages 的兼容服务 | 原 content index 保存在块 id；缺少 portable phase 时 Unknown | thinking 增量与 signature 增量共同累积；redacted 原样；tool use 保原 index/id/参数 | stop reason 与块闭合共同验证；message_stop 才发 finish |
| Chat Completions／`openai.rs` | OpenAI、Azure、DeepSeek、Qwen、GLM、Gemini、Ollama、Kimi／其它明确兼容端点 | content 始终正文，缺阶段 Unknown | reasoning_content 独立；工具参数按 index 增量；原 thinking 配置翻译保留 | `[DONE]` 与合法 finish 共同验证；非流式使用相同完整内容／工具校验 |

`complete` 只完成一个块。工具与正文混合不改变正文 phase；同响应出现早期 final
块也必须等完整响应、待工具回填和运行时后续校验。provider 不能宣布 TaskRun 目标完成。

Anthropic `end_turn/stop_sequence/refusal` 映射正常候选，`tool_use` 映射工具回合，
`pause_turn` 映射 `continue` 供原 ReAct 循环继续。`max_tokens`、context window
超限、未知 stop、异常 EOF、未闭合块、空响应和只有 reasoning 的响应均拒绝成功。
OpenAI-compatible 的 `length/content_filter`、缺 finish／DONE 和无正文／工具同样拒绝成功。

## 历史回传

Responses 回传 canonical assistant phase，不根据中文内容或工具有无推断；原 opaque
reasoning 的 id／encrypted_content／summary 保留。Anthropic 从同一 canonical 块
恢复 content 顺序、签名、redacted thinking 和原工具位置。OpenAI-compatible 请求剥离
Framework 的 assistant_blocks 元数据及其它供应商的 reasoning_blocks，保留其本协议
reasoning_content，避免向网关泄露私有扩展字段。

原生来源通过同一个 `ProviderOutputSource` 附着在 `AssistantMessageBlock`、
`ToolCall`、`DeltaToolCall` 上，保留 protocol、request_id、item_id、output_index、
content_index、content_kind 和原始 phase。Responses 回传按原 output index 交织
message、function_call 和 reasoning；同 message 的内容按 content index 分组，
`refusal` 仍为 `refusal`。未知 phase 在语义层保持 Unknown，在协议回传中保留原字符串。
工具的来源沿 Framework 原增量累积器进入最终 ToolCall，避免 Message 持久化时再次丢序。

`Message.execution_status`、`approved_assistant_blocks` 是运行时事实／派生呈现，
不属于 provider wire；所有适配器出站均剥离。OpenAI-compatible 入站也清除服务伪造的
同名字段、assistant_blocks 和 provider_source，再仅依据真实 response id、choice、
tool index 等字段重建来源；provider 输出不能伪造 Candidate／Settled。
这些字段当前仍需同源 P4 回放验证，源码不代表已获整体验收资格。

## 依据与证据边界

参考官方 [OpenAI 输出消息类型](https://github.com/openai/openai-python/blob/main/src/openai/types/responses/response_output_message.py)、
[Anthropic Message 类型](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/types/message.py)、
[DeepSeek thinking 协议](https://api-docs.deepseek.com/guides/thinking_mode)、
[Qwen 深度思考](https://help.aliyun.com/zh/model-studio/deep-thinking)。本轮实际取得的
blob／正文 SHA 及失败原件记录在主根 providers 执行证据，不把 HTTP200 地区不可用页、
下载超时或搜索片段当作协议正文。

回放通过本地 HTTP 调用真实 LlmClient，验证解析、身份、阶段、工具和错误边界；它不证明
真实 API key、模型服务、native GUI、IM／cron、SDK 精确 pin 或 main CI。示例与官网的
公共消息合同由 Framework／官网主写者同步，本文不添加第二套示例入口或对外终态定义。
