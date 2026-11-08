# ADR 0101：Framework 原生消息与统一执行终态

## 2026-10-29：响应观察和不完整响应承载

Root 已按完整原生消息目标批准 D1/D2，准确字段、空值与失败合同见所属框架[响应观察设计](../supreme/specs/provider-response-observation/design.md)。现有 `raw.extra` 和 output block 坐标都不能替代 response 级耐久观察。

在现 Message／DeltaMessage 各增加同一可选 `provider_response` 固定记录；ChatResponse／ChatChunk 不再重复挂载。记录只含真实协议／provider／request、model、stop、status、incomplete_details 和原生 usage。可空原件缺失与显式 null 区分；同请求增量是完整观察快照，usage 不双计、跨请求不串。现 projection、摘要和 File／checkpoint/Fork/import 沿同一消息保留，模型历史 wire 剥该观察并保必要 reasoning 回放。

`LlmError::IncompleteResponse { partial: Box<Message> }` 仅承载失败观察；不把 partial 当批准正文、成功 chunk、HTTP／ProviderError 或完成事实。Display 不输出部分正文，原 retry=false、Core AgentFailure 六字段和 SDK 有界投影不扩 raw。真实 provider error、HTTP、取消／资源／EOF 与 AgentEvent 结束权威不变；不新增 executor、Store、总线或状态机。该公共合同已批准，实际实现和各协议／消费者验收分别记录，本文不预授通过。

Chat 不完整工具片段即使尚无原生响应 ID，也必须保留原始稀疏 index。复用现 Source String 与 output_index：只在 D2 partial ToolCall 坐标中用空 request_id 表示未知原生 ID，响应观察仍保 None；实际 ID 后到时回填原请求全部坐标。候选的 Option 字段变更会连带所有 Source／SDK codec，而现有 String 与消费者已经能表达该未知值，故不改变公开形状或新增平行 index。空值不能充当 runtime／跨请求 owner 或成功回放身份；Responses 原空 ID 拒绝不变，依据与范围见响应观察设计。

> 当前范围说明：本页“旧变体暂留”、“默认wrapper”及“无runtime旧source兼容读取”的描述，均为已被ADR0104取代的迁移阶段记录；当前组合不再提供这些兼容路径。原机制/旧运行证据保留，当前切片按ADR0103/0104/0105执行。

## 状态

已接受，按用户 2026-10-27 明确指令实施；实现与全链路验收未完成。

## 背景

Framework 旧路径把 `think`/`final_answer` 当作控制工具，并用 `ThinkStart`/`ThinkEnd` 推断文本阶段。provider 已经能提供 reasoning、正文、工具调用、停止原因和原生签名；继续依赖控制工具会造成普通进度误归 reasoning、回答提前结束及跨 provider 信息丢失。

## 候选方案

1. 保留两个控制工具并继续扩展状态机：改动小，但继续复制 provider 能力并制造第二终态权威。
2. 新建独立执行器处理原生消息：可以隔离迁移，但违反现有 ReAct 单一执行权威，且会复制取消、重试、Guard、Stop 和结算。
3. 在现有 ReAct 主循环上增加 provider-neutral assistant block / execution status 投影，退役两个具名控制工具：保留通用生命周期，provider/EKO/SDK 各自只做薄适配。

## 决策

采用方案 3。`AssistantMessageBlock` 记录稳定身份、request 归属、顺序、phase、增量和原生 reasoning 数据；`ExecutionStatus` 只做运行观察。最终候选仍由既有 Guard、Critic、Stop Hook、持久化和 `finalize_run` 确认，`AgentEvent::FinalAnswer` 是兼容投影，不是模型工具调用。

## 取舍与影响

- 新执行不再默认注册或调用 `ThinkTool`/`FinalAnswerTool`，减少工具面与特殊分支。
- 原生 thinking 配置、签名、redacted/opaque block 和历史回传保留；旧日志只读适配。
- Provider 适配器必须无损填充 block 合同；EKO 负责 60 秒进度策略和呈现，SDK 负责协议 facade，不能各自判断结束。
- 现有公开 `AgentEvent` 兼容变体暂时保留，迁移完成后由消费者合流再清理读取兼容代码。

## 公开返回形式、模型传输与旧历史来源

`execute/chat` 的 String 与事件流属于公开返回形式；供应商 HTTP 流式/非流式是独立的
模型传输选择。保留默认流式传输，新增 `AgentConfig::model_streaming(bool)` 与
`ReactAgentBuilder::model_streaming(bool)`，运行快照携带该设置；只有显式 false 调用既有
`LlmClient::chat`。完整响应适配为同一 canonical 输入，保全部原生消息/推理/工具来源与
usage，复用原 ReAct、Guard/Critic/Stop 和 Managed 物理结算，不增加模型循环。
String 入口不再另持执行锁、租约或提前结算；execute 独立上下文、chat 历史语义保持。
回调沿同一 canonical 数据解释，不恢复旧 Token/Think 事件或普通文字 Thought。

2026-10-28 用户最新 Goal 明确开发期不承担旧版本兼容。旧内置 think/final 历史
来源 getter、旧控制断点归一与专用 File recovery CAS adapter 已随本切片退役；
不再根据旧消息参数、旧工厂版本或来源 hash 生成新的模型历史/最终候选。旧记录、
日志、固定 tag 与既有恢复包保持原件，不为旧资料缺字段新增解析、迁移或补成功。

以上是实际 Rust 公共 API 增量，包括 RuntimeConfig 的模型传输字段和 compression 类型
再导出；无新增 Message/Checkpoint serialized 字段或自动三语言合同。SDK 须单独核验
接受范围和正式 pin，不能预报零 delta。

默认 Core wrapper 可嵌套。`ExecutionStatus.adapter_generated` 是新增的进程内 Rust
来源标记，`serde(skip)`，不改变 JSON 合同或 canonical digest。只有合成投影可整体绑定
caller 身份；原生 Started 与末尾状态保原 source 身份，unknown 也按该 source 关联。
该标记避免把内层适配器合成的身份误当物理 owner。真实 runtime 状态为 false，只有旧
source 默认适配生成的状态为 true；它不授予完成或副作用许可。

原生消息携带更多可选回放数据后，`TurnInput::Message` 的 Rust payload 改为
`Box<Message>`，避免纯文本输入也承担整条消息的枚举大小。`TurnRequest::from_message`
继续接收普通 `Message`，driver 分支解引用后传递原消息；无 wire 或生命周期变化。
直接构造该公开 variant 的 Rust 调用方需显式装箱，SDK public inventory 单独记录此差异。
`TurnInput` 本身未实现 Serde，不是 wire 类型；不把其 Rust 布局检查称为协议 roundtrip。
依据为官方 [Clippy large_enum_variant](https://rust-lang.github.io/rust-clippy/master/index.html#large_enum_variant)：
枚举的最大 variant 决定其整体大小。此次只调整间接存放，不宣称已测性能收益。
内部 `ThinkOutcome` 与 mock 响应的同类大载体也只作装箱/解引用，不改变原消息或分支。

响应观察加入后，私有 `IterOutcome::FinalText` 的观察以 `Option<Box<ProviderResponseMetadata>>`
间接存放，构造后仅在原 driver 交付前解箱；Message／Delta 的公开字段及九字段序列化形状不变。
公开 testing `StreamChunk::Delta` 直接 Rust 构造改为 `Box<DeltaMessage>`，例如
`StreamChunk::Delta(Box::new(delta))`；全部合法 delta 原样保留，mock 消费时立即解箱。
这是真实 Rust 构造签名变化，消费者与 inventory须如实同步；同一已接受字段的值保持无损，
不新增 testing family、JSON 包装层或独立跨语言目录承诺。原 owner、停止、usage 和失败路径不变。

## 当前 typed 恢复与 context-view 派生

新执行与显式 Resume 均只沿原 RuntimeStateStore load/reconcile、严格 typed payload
解码、tool-call/result 配对与必要输入投影进入原 ReAct。无旧 pending-control fallback，
不会从历史 final 参数产生新候选，来源不明不降级猜测；runtime FinalAnswer 事件和
on_final_answer 回调仍由原 Guard/结构化/Critic/Stop/持久化/物理结算权威生成。

原 File CAS kernel、scope/runtime 锁、expected revision、epoch、replay/conflict 与
耐久 transcript apply/ack 不变。删除的只有历史来源检查、专用 recovery request/commit
及新建 _recovery_sources 迁移包的执行分支；已经保存的所有旧恢复原件保持，既有
路径不会因代码退役而被运行时删除、改写或自动迁移。

`derive_context_view_checkpoint` 从当前严格 payload 派生 context-only view/cursor，
保原 generation、transcript ordinal 和 context lineage。它由当前 runtime-only 保存
主路径继续使用，不是旧控制工具兼容机制，不可随旧模块整删。当前恢复仍拒绝真实
未配对 tool effect、投递债务和 stale hydrated base；取消/超时不代替真实物理结算。原React准备owner覆盖普通Store Load/Guard/准备
await，并将同一owner转交producer；旧归一化移除不允许caller drop释放仍active的
mutex/资源束。

原兼容专用成功测试随人类裁决 retire，不写 matched/pass，也不新增 ignore/skip。
旧模块中当前仍成立的严格配对、cursor损坏拒绝、context lineage、CAS replay、旧
hydrated revision保护、绝对deadline和typed unknown资源保有断言改由新正常managed
fixture覆盖。用户新注册同名think/final_answer普通工具仍保正控，不因所在旧模块误退役。源码候选与实际编译/运行验收分开，本ADR不预报新测试通过。

## 工具资源随同一 canonical 消息保留

只有真实 `ToolResult.artifact` 进入原 `publish_completed_call` 时，才复用
`ContentPart::ResourceLink/LinkedResource.meta` 携带 `echo_tool_output_artifact` v1。
同条 tool Message 保留原文本、call_id、tool_name 和 artifact；失败结果也保留真实资源。
没有 artifact 时保持纯文本，不从正文、相同 call_id 或路径猜来源。可选字符范围只定位
原 producer 的精确显示后缀，编码失败保原结果并记录诊断。

Parts 继续沿原 Context、Steer mailbox、checkpoint 与 provider adapter 传递；不新增
Message 字段、Store、executor 或终态总线。EKO 显示、编辑、权限和 workspace 复制策略留应用。
ResourceLink 的模型表示计入原预算，Raw 验证不提高窗口、分页或输出/safety 阈值。
SDK 独立核验新增 Rust API 与 adapter；本节不代表跨语言合同、正式 pin 或原生验收已通过。

## 同一原流的执行事实与投递回执

`AgentTurnDriver` 在 sink 关闭或失败后继续读取原 source 到 EOF，保留首个 typed producer
错误；晚到的通用合成状态不能抹去它。sink 失败不立即清除已经观察的正文，最终只有实际
完成才保留正文；尾部读取不增加尝试投递的序号。投递失败诱发的 legacy 取消及合成状态
不覆盖更早的 sink 失败；真实 native 终态仍独立记录执行结果与投递失败。

native source 的 unknown 只由同一 execution 身份的运行时终态解除，普通 FinalAnswer
和 foreign 终态不能代替。未提供运行时状态的旧 source 保留原兼容读取，所有路径仍等原
EOF 才返回 receipt。这里复用同一 driver、原 input lifecycle 与 source，不增加结束总线或 owner。

## Driver 首帧的执行身份与恢复游标

当 caller 未提供 execution_id、last_persisted_sequence 为0，且第一原始item为唯一
AgentOutputProjection 已验证的真实 Started、合同首因槽为空时，Driver 在首次构造
EventEnvelope 前只读该真实 execution，固定本次流的有效头。原 inspect/fuse 之后的一项
Peekable 只缓存同一item，不重复observe、不改变Started顺序；转换失败记原首因槽，仍保
sink拒绝后的原raw drain、物理EOF、原计时起点和owner。此处不增加公开DTO或执行权威。

caller显式身份保持原值和原冲突拒绝。None+非零游标或prefix-first流全程保None，不迟绑、
不修改既有头或event_id；调用方不得重置cursor绕过同scope持久前缀。0只是必要输入，Driver
不拥有历史为空的证明。同一物理source的不同turn各自建立原projection及execution，不能
复用physical context ID。此修复只覆盖Started-first的fresh流，不承诺prefix-first历史发布。

验收包含加强既有首帧测试、cursor0/41和None前缀字节/event_id不变、显式匹配/冲突、
prefix/非法首item、同source两turn，以及既有setup/sink拒绝/held EOF和资源回收控制。
2026-11-02的具名focused先取得原生产区下exact1新增外层身份断言的真实RED。迁移旧fresh
测试预期后，新test-only切点再次实际NoRun/list并取得0通过/1失败/退出101；保同一整个
tests区SHA，再采用最小生产桥取得exact1 GREEN。整个`runtime::turn_driver::`实际list55，
运行55通过/0失败/0忽略，497过滤；原强helper、恢复41/prefix None、显式冲突和原heldEOF
控制均保留并通过。定向Clippy零警告、整仓fmt write/check自然0，原失败与实际程序/FP/.d
分别保存在`.echo-semantic/local/framework-native-diagnostics-focused-20261102-r01/`。
这是Framework Driver定向资格，不替代App真实physical owner连续RunTurn、Cron、GUI、
独立SDK、完整工程矩阵或main CI验收。

## 参考

- 跨仓设计：`docs/supreme/specs/native-message-execution/design.md`
- 本轮已保存的 Anthropic/Codex 参考回执：`.echo-semantic/local/native-message-execution-20261027/references/`


## 默认最终评审与非最终普通正文分别授权

`verifier_enabled` 只决定最终候选是否交给已配置 Critic；EKO 的已配置普通模型会默认
安装 Critic，因此不能把该开关等同所有 Unknown 中间正文的整段 hold。原生明确
Commentary/PartialAnswer、Reasoning，以及最终候选的 Guard、结构化校验、Critic、
Stop、持久化和物理结算仍复用原权威，不新增发布策略开关、validator 或执行器。

正常完整模型响应已经含工具调用或明确协议继续边界时，effective phase 仍为 Unknown、
未指定 JSON 输出、且原 Output Guard 批准的正文与原文相同，才允许普通正文进入既有
AssistantMessage 与 approved 历史。工具或 continue 只证明它不是本轮最终候选，
不把 raw phase 改成 Commentary，不据正文措辞或工具日志制造自然汇报。

同 request/block 的晚到明确 phase 继续由 effective_assistant_phase 解析；显式
FinalAnswer（含先 Unknown、最后空增量标 Final）不借工具分支提前公开，无论最终
verifier 是否开启。JSON Object/Schema 的 Unknown 原文、Guard 拒绝或转换的原文
继续 hold；异常 finish、缺终止或断流的 partial-failure 路径保持原条件。

回归必须驱动原 ReAct producer 与真实已配置 LlmCritic，在工具入口屏障观察原 Unknown
正文已公开而 Critic/Stop/最终确认尚未发生，再检查原工具结果回填、下一主请求与单次
最终结算。关闭 verifier、晚到 Final、明确 continue、JSON、Guard、Critic 拒绝/错误、
Stop 继续和 Reasoning 均为相邻反控；不使用额外模型或把 prompt 提醒当作发布证据。


## 2026-10-28：有效 SSE 上游错误的统一运行期分类

用户已明确选择仅扩展一个 `LlmError::ProviderError` 变体，保存 provider、error_type、
原 `code: Option<Value>`、message 与 retryable。code 缺字段和显式 null 分别保留，未知
type／code 不按 message 措辞归类。它是运行期 Rust 公共增量，不是新的错误 bus、loop 或 Store。

不采用将 HTTP200 流内错误包装为 ApiError529、ToolError 或 InvalidResponse 的方案：前者
伪造传输事实，后两者丢失供应商明确失败与协议损坏的区别。Malformed payload 仍可沿原
解析错误拒绝；有效 Chat／Anthropic／Responses SSE error 和 response.failed 明确 error
走 typed cause，先于 pending finish／usage 处理，不能被后续 DONE 成功覆盖。

本次所有 provider 适配器一律 `retryable=false`。固定官方 SDK 只支持保 body 并 raise，
不能据此给 overloaded／rate-limit／api_error 自动重试许可。真实 HTTP429／5xx 仍按原
实际 status 和 retry kernel 处理；ProviderError 分类接原 predicate，不改变次数、退避、
取消或 circuit permit，不在已开始的流内重新请求模型。截断、停止原因和 runtime 最终
回答规则保持原义。

AgentFailure／SDK 仍用既有有界投影，不新增 rawValue/provider_code 等 wire 字段；
http_status 为 None，normalized cause code 为 llm_provider，原 type/code/message 只按原
预算呈现，不承诺运行期 Value 端到端公开。SDK 外层 operation Retryability 仍 Never，
可重试 cause 也不能授权重做已经可能产生副作用的 operation。EKO 沿原有限 RunTurn
失败路径，不另起重试循环。

采用后须分别验证 Core、真实 provider HTTP/SSE、ReAct 原 retry／流收尾、Subagent／
Workflow 实际 facade、EKO 原有限 RunTurn、SDK Host 有界投影。旧公开 API 可运行的
transport 反控先取得真实 RED；依赖新 variant 声明的合同在生产 hunk 后验证，不把
编译缺类型当产品 RED。

### 初稿验证状态（历史）

当前仅完成 ignored 候选、source review、格式与静态应用检查，Rust、消费者、实际 API
和新版 native 资格待各自真实执行；当前冻结批不热并。旧提案三类型 retry allowlist 已
取消，不把旧 static pass 当 runtime retry 通过，不自动晋升 SDK pin 或官网镜像来源。


## 当前组合对旧迁移阶段的替代

本文件此前的旧source兼容读取、默认状态合成和旧共享setter/cache描述是迁移阶段记录；
当前切片由ADR0103、ADR0104、ADR0105取代这些阶段承诺，不继续保留旧格式或默认成功
wrapper。原生Started/终态、原source物理EOF、首因、执行与投递分离仍由原权威判断。
进程内adapter_generated只用于拒绝非原source投影，已不存在默认adapter生成成功轨迹。

运行时FinalAnswer事件/on_final_answer、安全最终正文、String便利、非流模型传输、
当前typed cold/Fork/CAS/工具配对/副作用不重放和合理多Store能力保留。Provider原生
签名/opaque回放及当前第三方wire字段不是旧内部兼容，不据此删掉。组合候选包含
存储、配置和调用方迁移；所有源码资格与未来真实编译/运行/消费者/现场资格分别记录。

## 公共请求与工具错误观察的后继字段

统一接口已有 ToolChoice 但两个请求仍用 String，FunctionSpec 无法区分未指定和显式 strict，工具结果进入 Message 后无法保留 success 的错误观察。选择扩展原类型和既有 canonical 投影：请求复用 ToolChoice，函数新增可选 strict，Message 新增可选 tool_result_is_error；不增加 executor、store、结束状态机或跨产品策略。

备选是继续解析字符串、从正文猜错误或关闭 capability；这些会丢形状/事实或绕过已承诺能力，因此不采用。函数参数 strict 和最终输出 schema strict 各自独立；Responses 未指定函数 strict 维持旧 false 默认。provider/model/endpoint 的真实条件仍由既有来源与适配策略约束，协议兼容和模型别名不授全族能力。

后继影响包括手写 serde、File投影/managed import/冷恢复、原 ReAct publisher、typed final-only 请求及独立 SDK consumer。SDK 基础冻结后只由 consumers 组合交其唯一正式 owner。当前字段包为 GREEN-v02 上未编译候选，仅有静态格式/补丁检查；必须由 Frame/Root 在新冻结批中取得实际 RED/GREEN 与消费者证据。响应 metadata/截断 typed 观察仍待裁决，不能沿本段私自增加字段或改变 typed8 ProviderError/allfalse 与原重试内核。

> 上段“GREEN-v02 上未编译候选”和“响应 metadata/截断 typed 观察仍待裁决”是字段包初稿的历史状态，不是当前结论。D1/D2 已获批准；当前 Framework 工作区已经采用 ToolChoice、可选 strict、tool_result_is_error、provider_response 与 IncompleteResponse，响应观察合同以上文“2026-10-29：响应观察和不完整响应承载”及响应观察设计为准。各次编译、协议回放、消费者、SDK 正式 pin、原生现场与发布仍按准确来源和实际回执分别验收；不回写旧候选或旧失败记录。
