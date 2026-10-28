# ADR 0101：Framework 原生消息与统一执行终态

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

旧历史来源由既有 `PreModelContextProjector::recovered_tool_origins` 空默认方法提供，
新增运行期 `RecoveredToolKind/RecoveredToolOrigin`。EKO 的旧工厂判断留应用；框架仅验证
来源与参数合同并归一模型副本。原恢复/reconcile、checkpoint、cursor/CAS 和日志保持
权威，成对旧控制结果保为历史说明，不据旧参数重新发布 final。已核来源的旧 pending
控制按下述同一恢复路径派生后继；无法结算的真实调用继续暂停，不补结果、不重放副作用。本轮真实归档没有旧控制调用，
它只证明正常旧 managed 恢复，不能替代 pending 控制正控。

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

## 已知旧控制断点的恢复后继

严格 normal load 先沿原 Store 完成；只有结构或配对的 `SerializationError` 才进入
纯读检查。原 typed unknown、取消、deadline、IO 错误保持原样；当前记录本来严格合法
时也保留原错误，不用恢复 CAS 覆盖它。`inspect_runtime_state_for_recovery_with_context`
默认 `Unsupported`，只有能够保证不 reconcile 写入的后端或薄 adapter 才显式实现。

`prepare_pending_control_checkpoint` 只返回已核来源的只读准备值。自动 stream 与显式
Resume 在原 admission、execution mutex 和 close authority 下，按原 scope/version/cursor
核准来源；旧 final 参数仅为历史候选说明，当前模型输出仍经过原 Guard/Critic/Stop、
持久化与物理结算。真实 pending effect、投递债务、未知或 custom 来源继续由原 owner 处理。

`RuntimeCheckpointRecoveryRequest` 经 `commit_checkpoint_recovery_with_context` 进入
原 File Store 的同一 CAS kernel，保留 expected revision、epoch、锁、replay/conflict 语义。
覆盖前保全并核对完整原始 bytes、SHA/size 和 snapshot；`_recovery_sources` 只作不可变
恢复原件，不参与正常 runtime 扫描或执行。派生 checkpoint 是正常后继版本，旧 cursor、
digest 和 journal 不回写。取消或超时观察不得释放仍未知的原 admission 与资源。

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
