# ADR 0104：Agent完整typed输入与单一原生结算边界

## 当前状态（2026-10-29）

单一内核方案已在 Framework 正式工作区采用，尚未提交发布。完整冻结组合
`f327f63e34b7b4fea30d1c34f8559431fd2228172a2d427d8f1eac5912ee75c4`
含 2831 个文件；同源格式化／格式检查和
`cargo check -p echo_agent --features subagent,testing --lib --locked` 均自然退出 0、
编译零警告。同源四项响应观察运行时测试也已实际通过，覆盖工具／Guard／Stop、
用量、请求隔离及失败观察的冷读／Fork/import；它们不代替本 ADR 全部内核场景。
callback／Hook、examples、完整条件矩阵、EKO／SDK、真实 native 与精确 main CI
仍按各自来源和实际门禁验收，不能由根库编译或四项通过提前授予。

### 初稿阶段状态（历史）

状态：Root批准NoCompat目标的ignored组合候选。Framework当前producer、consumer、callback与fixture按原规则合流；跨仓owner提案未正式采用，本轮没有Cargo或运行资格。

## 背景

当前trait虽然有Message+invocation方法，默认仍能借Text/聊天→execute回退并保留却不处理history/cwd/tool上界；默认stream wrapper可依据FinalAnswer与EOF合成Started/Settled及重排身份。TurnDriver按是否看到Status选择两套完成语义，普通正文在旧模式能成为Completed。callback带ID默认又降级为旧无ID。这些是旧协议表面，不等价于当前CustomAgent、String、Store或非流能力。

## 候选方案

1. 保原默认多模式加更多flag：仍有约束静默丢失和真假终态混用，不采用。
2. 新建Agent运行请求/执行器或第二source owner：重复现有typed Message/invocation/ReAct，增加权威，不采用。
3. 复用现两个typed producer，String/raw/非流便利沿同kernel，只认原Status与EOF：采用。

## 决策

Chat与Execute各保当前上下文语义，以现chat_stream_message_with_invocation_context和execute_stream_message_with_invocation_context为primitive。默认不支持在source开始前明确拒绝，不压Message/Parts、不吞runtime/history/cwd/readonly/positive bound/cancel/resource/input-drain。String转换只增加Message::user；自定义Agent可以显式实现当前两个primitive，保合理能力。ReactAgent/AgentHandle/ProjectAgentAdapter已有覆写由其owner精确保留，不回灌整文件。

非流String只归并同一source，复用AgentOutputProjection，原EOF后验证真正Started/同owner终态/确认正文，保首个原Result错误和必要观察，不从文字完成。运输层FinalAnswer仍是运行时安全正文通知，不为Execution/TaskRun/Goal签成功。TurnDriver只真实Settled可完成，真实usage只LlmUsage，不依ThinkEnd；tail failure和sink failure不能被晚到成功洗掉。取消仍由原producer结算，不由投影器制造Status。

callback当前*_with_id必须接原call id；旧no-ID实现者先由F/SDK/EKO owner迁完再删方法。Token/Think enum同理，producer processor和所有实际消费者先迁；external setter原共享可变状态同ReactAgent snapshot/Handle/EKO adapter同步退，真实configured cwd不是兼容垃圾不能连删。event_envelope的current合同不能猜外部缺失metadata。

## 影响与迁移边界（初稿阶段历史）

当前三文件生产hunk与最终Core旧表面移除hunk分开交付，以调用表列明同期owner依赖。源与回归在F当前firstbatch期间冻结，本线只候选。原driver raw-only fixture须改为显式current typed producer与真实自身Status，原物理EOF/unknown/首因/资源/投递回归不得删；只旧wrapper自动成功、旧TokenThink/UI兼容要求由人类取消，按retired记录，不加skip/matched。

本ADR的Agent内核切片不拥有Store schema；组合的当前存储格式单独遵循ADR0103。SDK pin、EKO产品策略、安全/权限门控不由本切片变更。可选数据库Store仍合法，原body/Reasoning/provider签名/opaque保留；恢复和副作用不重放仍是新版本必须功能。

## 参考与证据限制（初稿阶段历史）

已按规则查官方OpenAI与Anthropic参考：web404、GitHubreset、OpenAI403、Anthropic区域不可用页，未取得正文。完整尝试回执references/，不以HTTP200或失败搜索声称跨产品共识。此取舍依本仓已存在primitive/collector和实际调用路径，不新造机制；外部实现对等性未验证。

## 验收与文档示例（初稿阶段历史）

tests-first定义当前String/typed/rich input、无终态/foreign/尾错、原取消身份/EOFguard、driver完成等反控；原Core output_projection/getter与F首批回归不改。本轮只有静态format/apply/afterSHA资格，实际Cargo与当前18新native定义、原矩阵必须F新窗运行。callback示例demo11的with_id函数hunk单独交F，列其它callback/macro/SDK同步；example原已有可执行链仍须owner同批编译。官网由其owner从正式源投影，不由本线跨写，未正式落地前不宣传完成。

## ReAct调用值与观察API的合流决定

旧 `set_external_context/clear_external_context`、`LegacyExternalContextSnapshot`、capture/swap/build helper 和 Agent 共享 external Mutex/current trace 缓存退役。现有 `AgentInvocationContext` 是唯一调用输入；plain入口先取默认值，排队前由原 close authority 准入cancel并写回同一值，admission/Guard/prepare/final snapshots均从它构造。历史、cwd、readonly/工具正上界、预算、取消、trace sink、delegation、lineage/uplink与匿名resource guards仍按原None语义复制；config conversation fallback保留，runtime.run_id为None是合法状态，绝不回填turn UUID/trace/conversation。

`Agent::current_run_id` 保留原公开签名。SDK `source_operations.rs` 的Session投影与该公开operation实际消费它，不能仅因CLI未调用而删除。观察值只放在现有ActiveTurn内的一个Option<String>，由原lease同incarnation安装和清理；没有active或原runtime.run_id为空即返回None，不持有完整context/resource guards，不产生第二owner/cache。流的transport observation身份可以单独用turn fallback，不能反写该业务run id观察值。

trace沿已有 `start_scoped_trace_run` 返回值显式写入对应快照。传最终admission config.conversation_id，防runtime conversation为空时丢配置fallback；append/finalize/CheckpointResumed均接相同trace值。手动压缩与standalone resume各有本操作scoped trace。Guard前managed恢复只把本次恢复事实暂存当前snapshot，Guard后以占位输入开trace再发布，不提前保存原用户输入。普通cold Chat/Execute由prepare透传同一snapshot并记录实际恢复；Store typed unknown/deadline/CAS错误原样返回，交原ReactStreamTurnOwner保债。

delegate_*便利API保留；plain默认configured conversation、runNone及独立cancel；显式runtime_context参数继续完整值传递，不从getter逐项重拼丢失isolation/lineage/uplink/guards。Core ToolContext原本不携isolation_id，本次不擅改跨层wire；该原能力边界单列由Core owner判断，不能虚称本组补齐了它。

公开差异：setter方法退休；snapshot from_agent仅取配置和默认调用，调用级输入须用现有from_agent_with_invocation；current_run_id由共享预置值收敛为当前ActiveTurn观察。delegate/resume/manual-compress公开签名保持；恢复/prepare的crate-private签名新增显式trace snapshot。没有新SDK wire字段或接受集。本组仅源码/格式/隔离apply资格，保全旧source和失败证据；当前Guard、trace、压缩、脱敏、失败隔离、资源和副作用控制已迁定义，未运行，不用取消旧setter专用验收冒充pass。
