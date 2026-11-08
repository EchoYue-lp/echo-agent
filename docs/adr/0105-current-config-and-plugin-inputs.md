# ADR 0105：当前配置形状与插件输入边界

## 状态

当前方案已在 Framework 正式工作区采用，尚未提交发布。完整冻结组合
`f327f63e34b7b4fea30d1c34f8559431fd2228172a2d427d8f1eac5912ee75c4`
含 2831 个文件；同源 `cargo fmt --all`、格式检查和
`cargo check -p echo_agent --features subagent,testing --lib --locked` 均自然退出 0，
编译零警告。该限定不代表所有权限输入／插件生命周期场景、全部 feature、examples、
EKO／SDK 消费、完整矩阵或真实 native 已验；后继按准确来源分别验收。

### 初稿阶段状态（历史）

2026-10-29 候选，按用户最新“开发期无需兼容”裁决与 Root 明确范围准备。只写 ignored
合流 Tree，不表示正式源码已采用或编译/运行已通过。父代理拥有唯一正式整合责任。

## 背景

PermissionMode 当前已有八个稳定 ID，但 FromStr/Serde 还接受旧名称、大小写与空白
归一。GUI writer 实际发送 canonical ID，SDK 三语言仍有旧别名 parser，需要所属
owner 同步；去别名不能改变权限决策、默认值或用户直接打开终端/MCP的行为。

Framework ModelConfig 与 SourcedModelConfig 的 config/facts 侧车分成两个形状，前者
进入 FrameworkConfig 时不能自然携带侧车中的 provider、exact-model 与 caller 来源。
这些来源事实及直接 context_window 选择是当前合理能力，不因 EKO 是否使用而删除。

A2A/LSP 已具名退役后，prepared/registry 仍专门发现、读取、冻结、hash 和缓存
lsp.yaml，只维持旧文件诊断/冻结义务。用户最新裁决取消该兼容义务，但保留插件当前
Skill/Hook/MCP/Agent/Tool、泛型 frozen document、generation、reload和rollback能力。

## 候选方案

1. 继续保留 alias、两种配置与 retired LSP 文档容器：延续已取消义务，不采用。
2. 去掉整个配置来源、context_window、通用冻结或插件生命周期：会损害当前能力，不采用。
3. 只保 canonical ID，单一 ModelConfig 承载 facts；插件只消费当前组件，混合断言
   迁到保留文档：采用。执行、权限、resolver、Store和插件发布仍各用原有权威。

## 决策

PermissionMode 只接受 id() 返回的精确字符串：default、plan、auto-edit、full-auto、
auto、bubble、dont-ask、strict。旧 ask/accept-edits/autoedit/acceptedits/fullauto/
bypass/bypass-permissions/bypasspermissions/dontask/strict-confirm/strict-confirmation
与大小写、首尾空白变体不再解析。八个枚举值、allows_write/requires_interaction/
uses_classifier和 PermissionPolicy 本体保持；这只收敛输入，不加任何产品权限闸。

ModelConfig 新增 facts: ModelFactInputs。with_model_facts 返回同一 ModelConfig，
resolve_model_profile_at 与 apply_to_llm_config读取同一来源输入。provider/exact-model/
有序 caller records 的 source/provenance/version/freshness 不压平、不重新猜测；
SourcedLlmConfig 仍是当前 client 构建和能力刷新所用的合理 typed 值，不按名字机械删。

当前 context_window 字段及其设置 Some/清除 None 语义保留，它仍形成同一 resolver 的
CallerOverride，provenance仍为 FrameworkConfig::ModelConfig.context_window；原记录
version标签即使含历史词也不是运行时兼容分支，不为文案删来源。清除直接 window 不
抹去 facts；清除 facts 不抹去直接 window。协议选择、温度、max_tokens和压缩设置不变。

移除 SourcedModelConfig Rust类型及其旧 {config,facts} Serde形状，不加兼容type alias。
ModelConfig 保留当前默认配置语义，拒绝未知字段，以免旧嵌套 config 被忽略后假成功。
这有真实 Rust public API/Serde形状变化，不能声称“无API变化”：新增 facts字段，
with_model_facts返回类型改变，旧侧车类型/方法退役；src/lib prelude由父代理同步。

插件不再专门 probe、read、freeze、hash 或缓存 lsp.yaml，移除 _retired_documents 与
LSP诊断去重。未识别的旧文件属于用户原件，不删除、改写、迁移或恢复执行，也不靠
旧文件内容改变当前 generation 身份。当前能力是否可用仍由现组件合同决定。

泛型 freeze_document 仍冻结保留的当前 subagent文档；同一普通文件检查覆盖错误类型、
UTF-8、消失和链接。该检查防本地误配 FIFO/device 阻塞框架，不增加执行权限门控。
Skill/Hook/MCP/Agent/Tool的声明、变量替换、规范 JSON、同代hash、缓存失效、依赖
顺序、wire/rollback、取消和清理责任保持原路径，不建立第二生命周期或来源reader。

## 影响：当前调用方与跨仓边界

Frame 当前 ModelConfig 构造除本文件测试外均使用 Default/FrameworkConfig默认；
src/lib prelude仍需由父代理移除SourcedModelConfig导出。SDK Host当前完整literal位于
core_profile/wire.rs::framework_from_wire，需要显式 facts: Default::default()；
现 SDK wire的context_window/config_version与凭据读取/擦除语义不变，不新增SDK字段。

SDK accepted-external-contract已有 ModelConfig现字段，尚未包含 SourcedModelConfig
或 ModelConfig::with_model_facts身份；新增 facts不会自动加入接受集或冒充三语言合同。
SDK owner独立核其当前Rust inventory/接受边界与三语言 PermissionMode parser/测试。
本候选不改SDK wire、accepted manifest、pin或依赖风险合同，未取得新的SDK wire授权。

EKO GUI使用mode.id()写出当前四个选项；旧normalize spelling由其owner去掉。产品对
internal plan/auto 的GUI呈现仍归EKO，不在本框架切片重新分类或设计六类工具。
具体 source proposals、精确base/hash和未采用状态保在本批owner-proposals中；本线不
写CLI/SDK正式文件。模型配置38章与官网镜像由所属owner按新来源同步，不造英文副本。

## 验证与验收变化

旧别名成功、旧侧车shape和 retired LSP冻结/诊断/identity义务retire，不新增skip或写成
matched/pass。当前八ID roundtrip/旧拼写拒绝、来源fact Serde→resolver→AgentConfig/
LLM投递、直接window与facts独立清除的断言保留或新增。

旧LSP夹具中的泛型文件错误/缓存/链接/只读冻结不变量迁到当前subagent文档；保留组件
的四类内容变动、依赖顺序、变量/配置hash、错误隔离、wire/rollback与清理测试仍完整。
本轮只可有ignored apply/字节/rustfmt及有限源码复审；Cargo/typecheck/实际回归/完整
矩阵、SDK消费者和native验收均待后继真实窗口，不借旧票标新快照通过。

## 参考与取舍依据

复用现 ModelProfileResolver/ModelFactInputs 的来源优先级与 freshness，不另造resolver。
配置形状和别名处理采用 Serde 官方 derive机制（rename、alias、default、
deny_unknown_fields）；本机已安装的serde_derive 1.0.229官方源码与当前Cargo.lock
registry身份/checksum核对，来源收据在本批references/local-primary-source-receipt.json。
官方网页检索不可用、直接网站403及GitHub连接失败均保为原事实，不声称已在线验证。
这里的兼容退役取舍由人类最新裁决决定；Serde只提供精确类型解码，无权限或产品政策。
