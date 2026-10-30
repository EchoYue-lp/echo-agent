# Anthropic 原生响应观察与不完整响应

本增量以已交 Frame 的 compat5（Anthropic `647cb225…d5b611`）为基底，独立于旧冻结包。公共合同唯一事实源为 Framework 的 [响应观察设计](../../docs/supreme/specs/provider-response-observation/design.md) 与 [ADR0101](../../docs/adr/0101-native-message-execution.md)；正式类型、State/Runtime和错误分类由Frame维护，provider只做协议薄转换。

## 同一消息中的固定观察

使用已有公共 `echo_core::llm::types::ProviderResponseMetadata`，只挂在 `Message.provider_response` 和 `DeltaMessage.provider_response`。ChatResponse/ChatChunk不重复挂载，不建立sidecar、Store或事件总线。

protocol取实际Anthropic协议，provider取实际配置身份；request_id只取服务原id，缺省/null保持None，canonical block为内部顺序生成的fallback不能假冒服务id。model/stop_reason/stop_sequence的缺省、显式null、未知字符串逐值保留，usage保存原对象及未知嵌套值。Anthropic没有原生Responses status/incomplete_details，这两个字段保持缺省；上游同名provider_response/execution_status/approved字段不能进入框架权威。

通过Core公开serde构造和校验记录，不导入私有presence helper、不复制公共DTO或validator。非法字段类型明确失败。非流式仍保原raw.extra，但新的固定观察属于同一个Message，可沿原历史与持久化运输。

## 流式完整快照与usage

message_start建立当前原生响应记录，message_delta按同一request更新并发出完整观察快照。缺字段保留已观察值，显式null保持真实值；usage同名计数替换、对象缺失键保留，未知嵌套对象按键更新，不能把累计output_tokens重复相加。重复usage-only更新仍有完整快照；新的adapter从空记录开始，重复/别的message_start拒绝，不让下一request继承旧model/stop/usage。

metadata-only chunk没有正文和finish_reason，不赋予成功资格。只有message_stop且原内容/工具完整性规则通过，才发正常协议终态和归一Usage。原生usage缺省或null不补造归一token计数。观察快照的接收和预算Usage的计算是两个合同，consumer不得对每个快照再累计usage。

## 不完整响应始终失败

end_turn/stop_sequence/refusal保其原观察并归一stop，pause_turn保continue；只含reasoning的pause仍继续。max_tokens、model_context_window_exceeded、未知/null/缺停止原因以及已观察响应缺message_stop的干净EOF，使用既有新增 `LlmError::IncompleteResponse { partial: Box<Message> }`。

partial保存已收到的canonical正文、签名/加密/unsigned reasoning、完整或尚未完整的工具参数及同一provider_response；其execution_status与approved_assistant_blocks始终None，不发布成功chunk、最终正文或工具执行。未完整工具JSON/签名即使被显式拿来重放，也仍由原参数/签名校验拒绝。已完成block不等于response/run完成。

真实SSE error保持原ProviderError的type/code/message及retryable=false；真实HTTP错误、取消、网络/解析错误仍沿原transport边界。不把它们改成假HTTP状态或自动重试，也不以D2吞掉真实ProviderError。IncompleteResponse的Display和AgentFailure有界投影由Core固定说明，不能打印整段partial；SDK既有有界错误字段不自动携带raw/partial。

## 历史、持久化与测试边界

发下一次模型请求时，provider_request编码剥provider_response和runtime/approved字段，signed/redacted/DeepSeek unsigned回传、文字/工具/Parts原序保持。State需沿同一Message project/restore和原digest/managed import/冷开保观察；provider没有新持久化权威。

`anthropic_metadata_tests.rs`覆盖33组原生fixture的流/非流字段存在性、unknown/null、usage不双计、别request拒绝、缺终态partial、真实ProviderError优先、伪造字段隔离、DeepSeek实际配置身份、partial工具不可回放及真实HTTP冷serde后下一请求。新类型是其编译前置，compile101不是行为RED。

另交Frame根crate集成测试片段 `consumer-tests/anthropic_metadata_cold_store.rs`：真实HTTP→provider→File managed import→全部本地store handle关闭→冷开→下一真实HTTP请求，逐值核观察、签名/加密块、Parts与wire剥离。该测试放根crate以复用已有State依赖，不向echo_integration新增依赖；Root/Frame在主目录内设置TMPDIR并纳入实际同源批。独立JSON loss片段只用旧Message serde，可作为字段丢失的old-API行为控制。

当前只交源码、夹具和回归定义，未运行Cargo或HTTP，未取得真实RED/GREEN、Store/native/SDK/consumer/mainCI资格。Frame实际共享载体/collector/State/error分类、源码402与完整Goal按原边界分别验收；旧compat5的源码审查票不扩为本metadata增量的执行票。
