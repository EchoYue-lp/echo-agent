---
title: 原生响应观察与不完整响应的无损运输
artifact: design
carrier: markdown
---

# 原生响应观察与不完整响应的无损运输

## 问题与目标

当前 `ChatResponse.raw.extra` 只留在非流式响应，`ChatChunk.delta` 与耐久 `Message` 没有同一响应级观察载体。原停止原因、显式 null、模型标识及 usage 扩展会在流式归并、上下文和冷恢复中丢失。返回 `InvalidResponse` 的不完整响应也不能让调用方检查已观察到的部分消息。

2026-10-29 Root 根据本轮完整原生消息目标批准 D1/D2。目标是在既有消息和错误体系运输事实，保持原 ReAct、Guard、Critic、Stop、取消及物理结算权威。

## 范围与非目标

通用载体、serde、错误分类、同请求归并、历史投影属于 Framework。各协议适配器只提取真实供应商字段；SDK owner 维护既有语言合同和有界错误投影；EKO 保留呈现与产品策略。

不新增 executor、Store、事件总线、状态机、自动重试或产品权限。观察存在不代表正文批准、供应商协议成功、源 EOF、执行 Settled 或业务目标完成。不记录请求正文、凭据、完整响应报文或任意顶层 extra map。

## 决策

在现有 Message／Delta、LlmError、ReAct 与 Store 权威内扩展下述唯一载体及适配边界，
由同请求观察和原生命周期共同运输；不引入平行执行或存储机制。

## 唯一公共形状

`echo_core::llm::types::ProviderResponseMetadata` 是同一响应的固定字段记录，通过现有 `echo_core::llm` 与 Framework `llm::types`／`llm` facade 导出。

| 字段 | Rust 类型 | 语义 |
|---|---|---|
| protocol | `LlmApiProtocol` | 实际使用的协议，不授模型能力 |
| provider | `String` | 实际配置的供应商身份，不由模型别名猜测 |
| request_id | `Option<String>` | 服务实际响应 ID；缺失不生成假 ID |
| model | `Option<Value>` | 原字符串或显式 null |
| stop_reason | `Option<Value>` | 原字符串或显式 null，未知字符串保留 |
| stop_sequence | `Option<Value>` | 原字符串或显式 null，不从停止原因推断 |
| status | `Option<Value>` | 原字符串或显式 null，不授运行时状态 |
| incomplete_details | `Option<Value>` | 原对象或显式 null |
| usage | `Option<Value>` | 原对象或显式 null，嵌套扩展逐值保留 |

`Message.provider_response` 与 `DeltaMessage.provider_response` 使用同名 `Option<ProviderResponseMetadata>`。`ChatResponse` 由 `message`、`ChatChunk` 由 `delta` 运输，不重复挂字段。现有 token 归一计数继续是预算／用量权威。

可空原件字段用 `default` 和私有 present-value serde helper：缺 key 是 None，显式 null 是 Some(Null)，Some(Null) 序列化时不能省略。字符串／对象字段的非法类型明确拒绝。不存在公共三态 enum、任意 response map 或第二份 sidecar。

## 不完整工具片段的坐标

Chat Completions 的工具增量以独立 `index` 定位；响应 ID 尚未观察到时，仍须保留稀疏的原始 index。沿用唯一 `ToolCall.provider_source`／`ProviderOutputSource.output_index`，不新增第二个 index、工具 sidecar 或状态权威。仅 D2 partial 中的 ToolCall，`provider_source.request_id` 空字符串明确表示未知原生 ID；同一 Message 的 `provider_response.request_id=None` 保持真实缺失，不生成替代 ID，也不把坐标塞入工具 id、name、arguments 或 usage。

该未知值只在原请求局部 map 内有意义，不能充当跨请求关联、owner、成功或可回放身份。实际原生 ID 后到时，对原请求 map 中全部工具 index 回填；已知原生 ID 改变仍沿原协议错误拒绝。Responses 回放对空原生 ID 的拒绝、Assistant/runtime 自身 ID 非空约束和不完整响应禁止执行工具的规则均保持。现 Core／SDK Source 字段都是允许空值的 String；Java Source 仅拒 null，非空 identity 校验属于另一层 Assistant/runtime ID，因此本次不改变公开字段形状。

依据固定 OpenAI SDK 源码 `fa67521` 中 `src/openai/types/chat/chat_completion_chunk.py`：工具 `index` 与可缺失的工具 call id 独立。该表示仅修复已知坐标丢失，不扩展完整响应的准入或成功资格。

## 增量、历史与恢复

增量中的 Some 是当前同一请求的完整已观察快照，不是跨请求 patch。协议内部按原 SSE 更新语义归并；缺字段保留已观察值，显式 null 是实际值，usage 快照不能累加两次。实际 request ID 已知后必须一致，迟到的其它请求记录不能覆盖当前请求。

既有 ReAct collector、think／tool／最终确认路径把同一观察保存在原 assistant Message。没有新观察的普通用户／工具消息保持 None；Guard 转换正文、Stop 继续、压缩、Fork 和 checkpoint 不得丢失同一消息的观察或据此补成功。

`MessageProjectionMeta` 只增加同名可选字段，原 project/restore、digest、File 冷开及 managed import 继续使用一个 canonical Message。固定 marker、真实 tool identity、generation、CAS、epoch 和原错误边界不变。缺失和 null 的差异进入原摘要，不新建元数据 Store。

向模型发送历史时，协议 wire 边界剥除 `provider_response` 及运行时身份／批准字段；必要 signed/redacted/opaque reasoning 仍沿原回传路径。服务伪造的同名字段不能进入 Framework 权威。

## 不完整响应与错误分类

在既有 `LlmError` 中增加 `IncompleteResponse { partial: Box<Message> }`。partial Message 包含同一响应观察和已收到内容，但不拥有 execution status、approved 内容或完成资格；不能伪装为成功 chunk、`ProviderError` 或 HTTP 状态。

Display 只给固定失败说明，不输出整段部分正文。分类与现有本地 `InvalidResponse` 同为不可重放失败，retry=false；Core AgentFailure 保原六字段和有界消息，不把 raw／partial 自动塞入 SDK wire。真实 SSE `ProviderError`、真实 HTTP status、取消、退避和 circuit permit 保持原内核。

已观察 metadata 后缺 message_stop／response.completed 或异常 EOF 仍失败；原进程、资源束及 source 必须沿既有关闭流程结算。unknown 不能由 partial、正文、另一请求或另一 execution 清除。

### 分类入口和首因

| 已观察边界 | 保留／产生的错误 | partial 与重试 |
|---|---|---|
| Chat 非流式或流式正常解码的 `length` 等既有不完整停止；Anthropic `max_tokens`／`model_context_window_exceeded`；Responses 明确 `incomplete`／`response.incomplete` | `IncompleteResponse` | 同请求已正确解码的原内容、工具片段、reasoning 和观察；retry=false |
| 合法已开始的 SSE 在正常 transport EOF 时缺原协议必需终止（Chat 缺 DONE、Anthropic 缺 message_stop、Responses 缺成功完成事件） | `IncompleteResponse` | 保已解码 partial；不得用最后一次 finish／正文补成功；retry=false |
| 未知 stop、pause／拒绝或未知 status 被原协议策略判为不完整，但 payload 本身合法 | `IncompleteResponse` | 原字符串保在观察，不因措辞猜成功、取消或自动继续；retry=false |
| HTTP 非成功状态、供应商真实 error／response.failed 中的有效 error、实际取消／超时 | 原 `ApiError`／`ProviderError`／取消／超时 | 不以 partial 覆盖真实首因，不伪 HTTP 或码，不新增重试许可 |
| 实际网络／IO 错误、坏 JSON／SSE、字段类型不符、无法正确还原消息的协议损坏 | 原 `NetworkError`／解析／`InvalidResponse` 等 | 不把损坏内容包装成已解码 partial，不用 D2 覆盖先发生的真实失败；原重试分类保持 |
| 从未建立合法消息或没有任何已解码响应的空输入 | 原 `EmptyResponse`／原协议空响应错误 | 不生成假 provider/request/message，不补 partial 或终态 |

真实 HTTP／供应商／传输／取消等已发生错误先于“到 EOF 缺终止”的派生诊断；协议只在没有该真实失败且原成功条件未满足时产生 D2。工具调用正常 finish、合法协议继续和原成功 stop 不因本记录新增失败。D2 的工具片段是观察数据，不授执行资格。

### 失败观察的耐久边界

D2 的完整 partial 由直接 `LlmClient` 错误调用方检查；它不自动进入 AgentEvent、SDK 有界错误或模型历史。ReAct 在既有失败结算前，只把同请求最后有效 `provider_response` 保存到原 Context 中的一条 metadata-only assistant Message：content 为空、tool_calls 为 None、reasoning／assistant_blocks 为 None、approved_assistant_blocks 为 Some(empty)、execution_status 为 None。没有观察则不新增消息。该消息只记录响应事实，不伪造 native 或失败终态；实际失败状态仍由原 finalize／projection 路径产生。

metadata-only Message 经过原 checkpoint／project／restore／digest 保存，复用现有持久化与资源 owner。绝不把未完成工具调用、部分正文或未批准 reasoning 片段作为可执行 assistant 历史写回；原 partial 在错误值中保留，不能从 metadata-only 记录宣称正文或工具片段已经耐久保存。后续请求在原协议 wire 边界剥除该观察，因此冷开、Fork、import 都不能据此重放工具、补正文或授权成功。

这里的 wire 排除针对整条 metadata-only Message，不能只删字段后留下空 assistant。精确识别上述 assistant／空 content／无 tool_calls、reasoning、assistant_blocks／approved Some(empty)／无 execution_status 且有 provider_response 的组合；合法的空正文加工具调用、signed/opaque reasoning 或其它角色消息不因观察字段被删除。

已有错误分支仍须显式接入这一保存时点，不能假定错误值会自然进入 Context。写入／结算失败继续沿原持久化错误、unknown 和资源保有规则处理，不将保存失败当作完成，也不吞掉原首因。流中此前已观察 metadata 的取消／协议失败使用同一最后有效快照，不依赖生成成功 chunk。

## 参考与取舍

已逐 SHA 核验固定官方 SDK 源：Anthropic `202db72c87d7c6eaa72129c67ce67fd343a3a20c` 的 Message、raw_message_delta_event、Usage、MessageDeltaUsage，以及 OpenAI `fa67521e1c59f11fb5cba7ef8b9b35b5fcac2ae4` 的 Response、ResponseUsage、ResponseIncompleteEvent。它们分别保留实际停止／usage 字段和 incomplete 响应。原件与成功回执在 Provider `contracts/resume-20261029/metadata-primary/receipt.json`；HTTP404／超时原件不是来源成功证据。

- [Anthropic Message](https://github.com/anthropics/anthropic-sdk-python/blob/202db72c87d7c6eaa72129c67ce67fd343a3a20c/src/anthropic/types/message.py)
- [Anthropic message delta](https://github.com/anthropics/anthropic-sdk-python/blob/202db72c87d7c6eaa72129c67ce67fd343a3a20c/src/anthropic/types/raw_message_delta_event.py)
- [OpenAI Response](https://github.com/openai/openai-python/blob/fa67521e1c59f11fb5cba7ef8b9b35b5fcac2ae4/src/openai/types/responses/response.py)
- [OpenAI incomplete event](https://github.com/openai/openai-python/blob/fa67521e1c59f11fb5cba7ef8b9b35b5fcac2ae4/src/openai/types/responses/response_incomplete_event.py)

复用现有 Message／DeltaMessage、Serde Value、LlmError、projection 与原请求 scope；标准库和现有依赖足以表达。选择固定记录优于任意原件 map，避免扩成不可审计的报文日志；选择既有错误变体承载 partial 优于成功 chunk，防止部分正文获得完成资格。

## 验收

- Message／Delta 的流非流往返保 stop 缺失、null、未知字符串与嵌套 usage 原值；非法字段类型拒绝，模型请求不回传观察。
- usage-only 更新不丢失、不双计；跨请求迟到观察不能覆盖本请求。
- 截断／不完整响应仍失败且不重试；Display 不泄漏 partial，Core／SDK 有界分类不扩 raw。
- 工具、Guard、Critic、Stop、最终确认和取消路径保观察但不增加完成事实。
- File 全 handle 关闭后冷开、Fork、managed import 与 digest 逐值相等；原 CAS／marker／身份反控保留。
- 真实 ReAct 在收到含工具片段的 partial 后失败，保存仅有观察的 assistant 记录；关闭所有 handle 冷开、Fork／import 后观察逐值相等，原工具调用次数为零、不进入后续模型请求、不出现 approved／execution 完成事实，bounded 错误不含部分正文。
- 三协议、实际 ReAct、SDK Host／三语言分别以准确同源结果验收；基础类型或局部 serde 通过不代替全链路通过。
