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

## Responses 请求方言与公共合同

Responses 的具名函数选择使用扁平 `{type:"function",name}`；公共 `ToolChoice` 的
Chat 序列化嵌套对象不直接透传。所选函数必须实际曝光。函数 `strict` 与最终
`text.format.json_schema.strict` 分别转换：前者 `None` 保持原 `false` 默认，显式
`true/false` 保留；普通 `ResponseFormat::Text` 沿默认文本，不伪造 JSON Schema。

工具结果 Parts 按原序转换为 `input_text/input_image/input_file`，文字工具输出保持
原生字符串；canonical 错误观察留在 Message 持久化，不发 Responses 未定义的
`is_error` 或运行时确认字段。文件保留原 base64 字节；PDF 使用对应 MIME，其他
文件沿既有 octet-stream 表示。具体模型接受的文件格式须按模型条件另验。

[DeepSeek Responses 官方说明](https://api-docs.deepseek.com/zh-cn/guides/responses_api)
明确 `deepseek-flash` 接受 user/developer 与工具输出中的图片，`deepseek-v4-pro`
不据此获得图片资格，两者不支持文件输入。该说明支持 `reasoning.effort` 与
`text.format`，不证明函数 strict=true 支持；无法兑现的显式配置在请求前返回错误。
DeepSeek 只发已声明 effort，不继承 Chat 的 xhigh 别名映射或请求 summary，用户
标识走其 `user` 字段。显式 temperature 在 reasoning 下不能生效时返回错误，
不静默抹去。默认 None 仍由模型处理；上层不得把产品默认误装成用户显式要求。

Responses 接受明文 reasoning 和 opaque 原件共存；摘要、明文 content 即使同文也
按原索引独立保留，只有 opaque 已携带的同源同索引摘要镜像去重。晚到 encrypted
原件经 Message 存储往返进入下一请求，不能覆盖先到 content。DeepSeek 不支持
encrypted/summary 的历史跨方言回传时明确报错，不丢原件后伪造成功。

以上实现候选的来源与实际验证分列：官方 schema 只证明字段形状；源码审查、格式与
补丁应用不代表 Rust、真实 HTTP、live API、ReAct 最终验证或 EKO/SDK 消费者通过。
完整 response.completed、工具优先、进度继续、取消/异常EOF、usage、最终 Guard/
Critic/Stop 及唯一发布仍沿原框架路径验收；provider 不增加终态权威。

## Responses 响应观察与失败 partial

Responses 在同一 `Message.provider_response`／`DeltaMessage.provider_response` 运输
Framework 唯一 `ProviderResponseMetadata`。从真实 response 外层提取 actual id、model、
status、incomplete_details 和 usage；不采用供应商伪造的同名观察／runtime 扩展。实际配置
provider 与模型别名分开；服务未给 id 时，观察的 request_id 保 None，不把 canonical
块的 fallback id 冒作服务 ID。可空字段区分缺失与原 null，未知字符串与 usage 内嵌扩展
按 JSON 值保留；不承诺原 HTTP 空白、key 顺序或所有顶层未知字段。

SSE created/in_progress 等只有观察更新的边界也可给 delta，但不发 success finish。
每份 Some 都是同请求完整观察快照：缺字段保持、显式 null 覆盖，usage 原对象替换而不
叠加；归一 Usage 仍沿原预算路径，不能把多份观察重复计入。不同原 request id 被拒绝，
独立请求各持自己的适配器；观察字段不新增结束状态机。

合法不完整响应、未知状态按原协议不允许成功及正常 EOF 缺完成时，用既有
`LlmError::IncompleteResponse { partial }` 保存同请求已正确解码的部分消息和同一观察。
partial 不带 execution/approved 完成资格、不自动发布或执行工具。opaque 和同 item 的
公开 summary/content 都要保留；晚到失败响应缺先前 opaque 时保同源已观察原件，文字、
phase 或 encrypted 冲突仍为协议错误。真实 ProviderError、HTTP、网络、取消等原错误
先保首因，不用 D2 覆盖；Display 和 SDK 的有界 AgentFailure 不附整段 partial。

失败观察的耐久保存由原 ReAct/State owner 处理。协议模型历史只调用共同
`Message::is_provider_response_observation_only()`，整条跳过由
`Message::provider_response_observation()` 构造的精确 metadata-only 记录；不留下空
assistant wire，不误删合法空正文带工具／reasoning 的消息。普通消息剥观察字段，原
opaque／明文／摘要和工具回填保持原协议次序。partial 正文本身不会因观察记录落盘而
获得耐久、批准或重放资格。

## Chat Completions 响应观察与失败 partial

Chat 沿相同的 Message／Delta 记录运输真实响应 id、model、finish reason 和原生
usage；缺字段、显式 null、未知停止字符串及嵌套用量扩展分别保留。usage-only 事件更新
同请求观察快照，成功 finish 仍等待合法 `[DONE]`，不会因多份快照重复计入归一用量。
真实 SSE provider error、HTTP、取消、网络与协议损坏保原错误首因，不被 D2 改类。

合法截断或缺必需终止事件时，D2 partial 保正文、reasoning 和原工具片段。原生响应 ID
尚未出现，也保工具的原始稀疏 index；仅此失败观察的 Source.request_id 空串表示未知，
响应观察仍为 None。实际 ID 后到时回填原请求全部工具和正文 Source，已知 ID 改变仍拒绝。
空身份不能用于成功回放；成功响应不得生成替代原生 ID 或靠空 Source 降级通过。

历史出站复用 Core 唯一 metadata-only 判定，整条排除失败观察、剥普通消息的观察字段，
保合法空正文带工具或 native reasoning 的消息。失败 partial 的正文和工具不因冷读、
Fork 或导入变成可批准内容；实际保存和结算仍归原 ReAct／State owner。

当前协议候选仅取得源码审查及格式/机械补丁证据；Core 载体的独立通过不代替协议
回放、真实 ReAct/Store、SDK/三语言、GUI/IM/cron、live 或完整工程门禁。
