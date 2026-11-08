# Anthropic Messages 请求与响应适配

本说明记录compat-v2基础；后继D1/D2已获Root采用，本轮响应观察合同与错误运输见[原生响应观察](providers-anthropic-response-observation.md)。正式采用由 Framework 唯一 writer 完成。协议适配只运输来源、原序、内容和错误观察；ReAct、输出校验、Stop Hook、持久化与运行结算继续拥有执行结束权威。

## 已有公共合同与映射

复用 `ChatRequest.tool_choice: Option<ToolChoice>`，不接受 JSON 字符串模拟类型。`Auto`、`None`、`Required`、`Function { name }` 分别编码 `{"type":"auto"}`、`{"type":"none"}`、`{"type":"any"}`、`{"type":"tool","name":"…"}`。指定函数必须存在于本次曝光工具；Required/Function 无工具时报错，None 可无工具。

`FunctionSpec.strict: Option<bool>` 独立编码到 `tools[].strict`。None 省略，false/true 分别保留。工具 strict 的支持事实与最终输出的支持事实独立。当前完整官方文章可证明具名原生 4.5 范围；v2 公共合同尚无独立 strict 能力事实，其它端点/模型即使声明最终 structured_output=true，也不能据此开启 strict。它与最终输出的 `ResponseFormat::JsonSchema` 是两个独立请求，互不替代。

`ResponseFormat::Text` 沿默认文字协议省略 format。`JsonSchema` 在已验证模型上编码 `output_config.format = {type: "json_schema", schema: …}`，不附加 OpenAI 专属的 name/strict 层。原 schema 不做有损重写；非对象 schema、strict=false、没有原生等价证明的 JsonObject 明确报错，不拿提示词模拟协议保证。更细的服务端 schema 约束由原 API 错误通路返回，不在 adapter 另造 JSON Schema validator。

## 模型事实与证据边界

官方 SDK `anthropics/anthropic-sdk-python` 精确提交 `202db72c87d7c6eaa72129c67ce67fd343a3a20c` 的完整 schema 证明了 tool choice、工具 strict、output_config、tool_result.content blocks 和 is_error 的 wire 位置；单凭 SDK schema 不证明某一模型支持。

官方完整文章 [Structured outputs on the Claude Developer Platform](https://claude.com/resources/articles/structured-outputs-on-the-claude-developer-platform) 的 2026-02-04 更新明确列出 Sonnet 4.5、Opus 4.5、Haiku 4.5 的 GA 支持。默认官方 `api.anthropic.com` 端点只对这三个具名家族的明确别名/版本开放已证明的 schema 能力，不推断未来 Claude、代理或其它兼容服务。其它最终输出配置须沿现 `ModelProfileResolver` 注入 fresh exact model / caller fact；过期、其它模型或被后继 false 覆盖的记录不授予资格。适配器能编码 schema 与当前模型支持 schema 分别表示；不能关闭全局能力来代替实现。Auto 在其它配置需要 fresh exact 工具支持，None 需要对应的 fresh exact supports_tool_choice_none。Required/Function 在未知兼容端点没有独立强制选择能力事实可依，当前只开放已证明的原生 4.5 与 DeepSeek 具名端点/模型组合；普通 tools 支持不冒充强制选择证据。

4.6、4.7 及其它模型的 schema／strict／采样组合没有由这份文章取得完整支持证据，仍需精确支持来源。thinking 方言继续由原 resolved profile 选择；本说明不修改 Core 的模型目录。官方文档站本轮 200 响应实际是地区不可用页，搜索工具返回 404、GitHub rate limit 与 raw timeout 都按原失败保留，不作规范正文。

## Thinking、effort 与采样

`effort` 只位于 `output_config.effort`，不再写 top-level effort。预算方言发送 `thinking.type=enabled` 和原预算，预算必须至少 1024 且严格小于 max_tokens；不会 clamp 显式预算。adaptive effort 方言发送 `thinking.type=adaptive` 和离散 effort，不能把精确 BudgetTokens 偷换成近似 effort。明确关闭编码 `thinking.type=disabled`，不把 omission 当关闭。没有该方言或 adaptive-only 显式控制尚未核实的组合报错。

原生 thinking 与 Required/指定函数强制选择不能组合；已证明的 auto/none 仍按原样编码。原生显式温度须有限且在 [0,1]；thinking 与显式采样未取得完整组合证据时明确报错，不能删除 temperature 制造成功。用户可选择省略采样或明确关闭 thinking；adapter 不替用户改配置。

## 富工具结果与错误观察

`Message.tool_result_is_error` 只来自真实工具结果观察，独立于 TaskRun 或 final。None 不推测成功，false/true 均持久化并编码原生 `tool_result.is_error`。`MessageContent::Parts` 逐项变成 tool_result.content 数组，文字、图片、PDF、UTF-8 文本文件和 ResourceLink 保原序；不会经 `as_text` 吞媒体。

图片支持原生 base64 jpeg/png/gif/webp 与 HTTP(S) URL；File 的 PDF 保 base64 document，图片文件转换 image，已知文本文件完整 UTF-8 内联。无法确定表示的二进制、坏 base64 和非 UTF-8 文本明确报错，不伪名称占位。ResourceLink 沿现有确定性 model_text 保存所有描述字段。相邻并行工具结果合入同一个 user 消息，保输入顺序和 tool_use_id；缺 ID、孤儿、重复或工具未回填就进入下一消息均拒绝。

签名 thinking、redacted thinking、文字与多 tool_use 沿现 provider_source 保原 request / content 顺序；冷 serde 往返仍使用原始 assistant_blocks，而非运行时 approved 呈现投影。assistant 的不支持媒体与 system 非文字内容也明确报错。当前统一接口未承诺的 server/computer/browser tools、files API ID、top_p/seed/stop_sequences 等不在本兼容子项扩张。

## DeepSeek 的独立 Anthropic 兼容方言

[DeepSeek 官方 Anthropic 兼容正文](https://api-docs.deepseek.com/zh-cn/guides/anthropic_api) 明确指出 `api.deepseek.com/anthropic` 的 claude-opus* 映到 V4 Pro，其它 claude* 映到 Flash。必须先按端点识别方言，不能因名字含 Claude 就赋予 Anthropic schema/签名资格。

DeepSeek 的 none/auto/any/tool 按原生映射保留；thinking 用 enabled/disabled，effort 位于 output_config，显式 temperature 支持 [0,2]。其思考模式正文明确默认开启，effort 的服务端离散映射保持请求值；精确预算因服务忽略 budget_tokens 而报错。thinking+forced 的组合尚无完整支持证明，明确报错。

该端点没有 native format/schema 或 strict 工具保证，不接受此类显式请求。document/redacted_thinking 不支持；is_error 被服务忽略。后继已明确：Some(false)保留canonical/State观察，但在DeepSeek边界等价转换为普通非错误tool_result默认表示；Some(true)仍在HTTP前明确拒绝，None未知维持原普通结果。该精确转换见[成功工具默认表示](deepseek-anthropic-tool-result-default.md)，不能拿普通成功请求冒称原生错误标志已支持。

DeepSeek thinking 没有官方 Anthropic 的非空签名义务。无签名/空签名以已有 provider-owned `ReasoningBlock::Opaque` 保存原 native thinking 对象，缺省和空串区分，按同方言回传，不伪造签名。原生 Anthropic 仍要求有效签名，外来 opaque 数据不能跨协议冒充 signed。DeepSeek loopback 用真实服务 URL 分类、仅 DNS 定向本机，绝不访问真实 API；缺省与显式空签名分别有双模式冷重放控制。

## 停止、错误与验证

end_turn/stop_sequence/refusal 归一 stop，tool_use 归一 tool_calls，pause_turn 只归一 continue。pause 即使纯 reasoning 也保持继续，真实工具参数/身份仍校验；截断、空终态、reasoning-only end_turn、异常 EOF、未知 stop 和缺 message_stop 均不能成功。SSE 错误沿 GREEN-v02 已有的 reported_provider_error 产生 LlmError::ProviderError，保留原 type、code 的 JSON 类型（含缺省/null区别）和原 message；全部 adapter retryable=false，不捏造 HTTP 状态，不从 busy/overload 措辞授予重试。取消继续复用现 transport，不新增 executor 或重试循环。本接口只保存必要错误观察，应用日志仍不能额外输出请求凭据。

实际非流式 HTTP 通路将原 JSON 直接保入现有 raw.extra，保留 model、stop_reason、stop_sequence 的缺省/null/字符串及原 content/usage/顶层扩展；不会先经 typed 反序列化再重序列化补造 null 或丢掉 citations。只调用内部 convert_response helper 的 typed 样本不具有原 JSON 存在性资格。raw.extra 不随 response.message 自动持久化，因此这项修复仍不等于全链无损。当前共享 ChatChunk 只容纳归一 finish_reason 和已建模 usage 计数，不能无损承载原生 stop_sequence、end_turn/stop_sequence/refusal 区别和任意 usage 扩展；独立 strict/forced-choice 支持事实也缺少共享合同承载。这些仍是待共享合同补充的明确缺口，不能拿 provider-local 暂存宣称跨层无损。provider 不自行新增平行 Core 类型。

新增 `anthropic_compatibility_tests.rs` 使用真实 loopback HTTP 捕获生产请求，定义双模式三轮工具、strict/最终 schema 分离、富结果与错误位、签名与加密块冷重放、原序、usage、pause、截断、EOF 和 body 中取消，以及 DeepSeek 别名/无签名方言回传。另外真实 HTTP 定义覆盖四种选择×三种 strict 值×流/非流 24 组请求，区分 custom final 支持与工具 strict，并拒绝未知 gateway 假借 Claude 名字获得能力。测试定义不等于已执行：本线禁止 Cargo，真实 RED/GREEN 由 Frame 持同源批次完成。live、EKO/SDK 消费、正式 source/main/CI 和完整 Goal 分开验收。

文档/示例/官网同步：本子项只交中文协议说明候选和可执行回归定义；公共 ADR、正式 examples、EKO/SDK 与官网由各自唯一 writer 合流，不能在冻结期跨权写入。未新增依赖、manifest、锁或 vendor。


## 2026-10-29 受控恢复组合

本候选采用唯一 GREEN-v02-fixture-equality 输入（manifest SHA `4ce68d8dcdfa13680f0df5cd8b4d1da3ed791bd7c096153178724187c5bed215`，入口SHA `831d71c2f5b95e4713e3924c92472fb24c1c7920e62b1364633c403b39e199b1`）。该树的 typed8 已存在；本线只逐函数组合 Anthropic兼容/typed error，不回写旧正式F684文件，更不替换Framework的react/mod或重试循环。

新增 anthropic_resume_tests 将F684可编译的旧API行为控制、GREEN-v02已选ProviderError控制与公共18新API回归分开。原12兼容定义与新增定义均未运行，不将源审/格式/apply当作真实HTTP/RED/GREEN通过。空redacted payload的流/非流接受规则已统一为错误；原文/签名/加密块原序沿同一Message冷serde重放。

原 stop/usage 元数据共享字段已给 contracts 最小请求并抄协调者。无共享运输字段前，本线不增加 sidecar、不把stop_sequence塞入reasoning summary，也不把截断伪造为ProviderError或成功结果。共享字段批准、持久化接线与正式同源验证仍须分别闭合。外部source402及live/consumer/SDK来源/native/mainCI/完整Goal继续保持未验。
