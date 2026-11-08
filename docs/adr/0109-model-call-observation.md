# ADR 0109：原模型调用的可选观测

日期：2026-11-07。状态：实现候选，验证与产品验收分别记录。

## 背景与候选

`LlmUsage` 已携带 provider 用量，但缺少可比较的源流时段。用 UI 收到批准块的时间、字符数或整个请求耗时计算 tok/s，会把 Guard 缓冲、首字等待和工具执行混入分母。候选为 UI 估算、provider 专有时间字段、原消费循环的单调时钟观测；采用最后一项，provider 专有信息仍保留其原来源。

## 决策与边界

在 `LlmUsage` 增加可选 `ModelCallObservation`。字段缺失保持缺失，不等于零时间；已有 JSON 无此字段仍能反序列化。它记录本地调用与尝试身份、请求结果、相对首末有效输出、采样数及计数/时段是否可比较。身份不得回填或替换 `ProviderOutputSource.request_id`。

计时在原模型消费循环、批准块释放之前采集。正文、公开推理和工具名/参数的有效源输出计入时段；角色、ID 和纯元数据不作为输出。多 choice、opaque 或无法证明覆盖的输出拒绝计算速度。仅完成、至少两次有效采样、正时长且计数同口径时，提供 `completion_tokens × 1000 / (last − first)`。请求总耗时单独保留，不充当 decode 时长。

失败或取消时，如原 provider 已报告用量，沿原事件保留该用量及失败观测；漏报保持 `usage_reported=false`。观测不授予最终答案、保存确认、TaskRun 终态或重试资格，不改变原失败路径，不增加另一个执行器或存储 owner。应用负责累计范围、展示和可恢复投影，框架不硬编码产品字段或品牌。

## 取舍与影响

参考 ZCode 与 DeepSeek 官方 Harness 的原 usage 记账方式，保留统计与显示生命周期的区分。源流时间是客户端观测，不能当作服务端纯解码基准。旧 producer 和 Subagent 专有事件未携带关联时明确未知；SDK 独立精确 pin 不随本改动升级。

公开事件的构造、完整匹配和序列化消费者须同步检查；现有 learning examples 仍使用 `..` 消费 usage，纳入对应编译验证。无需新增模型工具或 UI 示例。此 ADR 不证明实际 provider、原生 App、远端 main CI 或发布通过。

## 独立审查后的失败用量与重试补全

已确认的原合同要求 `ChatChunk.usage` 只在供应商终止屏障之后公开。Anthropic 的 metadata 阶段及 Chat 的 DONE 前都可能已经报告计数，Responses 的不完整事件也能携带计数。直接提前填原 `usage` 会改变其它消费者的记账合同；仅给 `IncompleteResponse` 加字段又不能覆盖主动取消和原 `ProviderError`，均不采用。新增 chunk 观察字段会触及所有构造和转发，也没有必要。

参考实际安装的官方 `@anthropic-ai/sdk` 0.129.0：`MessageStream` 为逐事件提供当前 snapshot，累计 usage 字段出现时替换而非相加，`message_stop/finalMessage` 保持独立终止边界；DeepSeek 官方 Harness 0.2.2-rc.1 的 Messages adapter 同样维护累计快照、只产出一次 final usage。官方 SDK 参见 [MessageStream 源码](https://github.com/anthropics/anthropic-sdk-typescript/blob/main/src/lib/MessageStream.ts)。本次实际核对的是本机原安装包内对应版本源码及其 SHA，网络查询失败没有冒充在线核验。

采用 `LlmClient::reported_usage` 默认纯查询：通用 trait 默认返回 `None`，没有 I/O、请求、重试或状态 owner；原 adapter 将已经核验来源的 `ProviderResponseMetadata` 及显式 previous typed `Usage` 交给自己既有的规范化器，返回可选 typed `Usage`。Anthropic 复用同一个计数合并 helper，Responses 复用 `ResponsesUsage → Usage`，Chat Completions 复用原 `Usage` 解码。协议或配置 provider 不匹配、没有已报告计数、缺字段或不可表达时，不得伪报 known；已报告的零与缺失保持不同。原 native/raw metadata 不添加字段或改写，原 terminal usage、finish、错误和发布屏障不变。root 不解析 vendor JSON，观测失败也不能替换原 provider 错误。

previous 只属于当前 attempt，在原消费循环接到已核 metadata 后替换更新，不跨重试复用。它保留原 `Usage` 基础字段及缓存分项，不是已经 inclusive 的 UI 总数。Anthropic raw 的显式 null 必须原样保留，而其 normalized 规则会忽略允许的 null、保留旧计数；因此不能每次从零重建，也不能把已 inclusive 的总数当 raw 再加缓存。失败/取消只消费最后已知的 typed 快照一次，不逐 metadata 加账；明确零仍替换旧正数。

全仓查询确认生产 `LlmClient` 实现只有 `OpenAiClient`、`AnthropicClient`、`ResponsesClient`；`LlmConfig::build_client`、Agent builder、模型工厂、Guard/压缩/分类器直接保留该 trait object，没有生产转发 wrapper。其余实现是具名测试模型或测试观察 wrapper，未声明该能力时保持未知；不自动改变 SDK 独立 pin。以后增加生产 proxy 必须无损转发此方法，并保留其配置来源校验。

逻辑 call ID 绑定一次原模型步骤；原 `retry_llm_call` 的每次实际 `call_fn` 执行获得本地 attempt ID 和独立起时，失败也沿原 `LlmUsage` 发一次已知/未知消费观察。成功获取的流继续使用该 attempt 的时钟与身份，不把重试等待拼入其 decode 时段。原 retry 循环、退避、取消和熔断决定不改；最终外层错误不能再把最后一次尝试重复记账。必须直接验证“两次失败、一次成功”为一步三次尝试，前两次未知覆盖保持未知，末次只计一次。
