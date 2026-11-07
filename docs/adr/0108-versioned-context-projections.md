# ADR 0108：有版本运行事实与稳定缓存前缀

状态：已实现，验证范围见本次工程回执；不代表真实模型命中收益验收。

## 背景与候选

在模型每步 prepare 时替换 system/history 边界的运行事实，会使其后的已见历史失去前缀复用。仅改成 tail replacement 仍会删除历史中的旧消息；永久保护每步胶囊会让压缩无法回收。DeepSeek Harness 的历史内更新、ZCode 的稳定/动态分层，以及 [DeepSeek 缓存说明](https://api-docs.deepseek.com/guides/kv_cache) 都支持先保持历史前缀，再独立验证外部缓存收益。

## 决策

扩展既有 `ContextProjection`，保留 `replacing` 的固定指令 generation 语义；`versioned` 在模型安全点追加运行事实。内容版本只与最新版本比较，因此 A→B→A 会追加纠正，同版本重入和恢复后重投影去重。生产者必须使用稳定 marker，并在来源消失时发送 `message: None` 的显式失效版本。首次无值不制造空消息。

版本信封由框架拥有，checkpoint 随原消息保存来源和版本，不增加另一套 store。只有每个 marker 的最新快照受保护，旧版可由原自动/手动 compaction 回收；`PreparedContextTransform` 同时交换消息和去重状态，失败回滚不留下半提交。信封说明新快照仅取代同来源旧快照的当前含义，原始用户请求及真实指令权威不变。

动态内容的角色由宿主明确选择；框架不会自动把 System 降为 User。规则变更允许新 generation 和正常冷 miss。缓存布局只识别带合法版本的通用信封，原始正文中的 `[runtime_context:` 不再作为缓存分段依据。动态 User 消息不推进 Visibility Horizon 的真实用户回合计数。Subagent 继续过滤框架信封，不继承父作用域的当前事实。

三协议在实际请求构造完成后调用 `cache::wire` 摘要。工具完整字段、parts、tool calls/results、需要回传的 reasoning，以及有效请求控制参数均参与规范化；数组保持顺序。Anthropic `cache_control` 单独计算摘要，移动断点不冒充正文变化。普通日志只输出摘要、数量和协议名；隔离对照的首差异路径使用成员序号，不输出任意用户 JSON key 或正文。`common_component_bytes` 是完整相同组件的规范 JSON 字节数，不是 token 数或 HTTP 字节 LCP。无全局请求缓存，不跨会话保存正文。

Anthropic [官方 SDK 合同](https://github.com/anthropics/anthropic-sdk-python/blob/main/src/anthropic/resources/messages/messages.py) 明确允许消息正文的字符串简写与单个 Text block 等价。本次核验 blob 为 `013ae7a6f38caf473d6137f232b042a97c763a15`。语义诊断只在消息自身 `content` 边界统一这两种表示，不合并多个块，也不忽略块的其它字段、顺序、工具 JSON 或 reasoning 签名；另列 `wire_representation_hash` 保留实际 body 的表示差异。语义前缀相同不表示原 HTTP JSON 字节相同。

## 影响与取舍

### 信封格式的发布边界

已提交的 v1 只定义 marker 头，marker 后的全部内容都是 Boundary 正文；
即使正文第一行是 version=1、version="x" 或 version=""，也不能据此推断 Tail。
本轮早期未发布候选把 Tail version 插入同一 v1 形状，产生了不可逆歧义；
该候选不构成需要兼容的已接受协议。

因此新写入使用 v2，头部显式写 placement，并以空行结束。Boundary 不含版本字段，
Tail 必须提供非空 JSON 字符串版本，未知/重复字段、未知 placement 和未终止头部均无效。
头部终止后不再搜索正文元数据：

~~~text
<echo-agent-context-projection-v2>
marker="config"
placement=boundary

version="x"
这两行仍是完整正文
~~~

~~~text
<echo-agent-context-projection-v2>
marker="runtime-state"
placement=tail
version="semantic-version"

当前来源快照正文
~~~

该边界参考 [RFC 5322 第 2.1 节](https://www.rfc-editor.org/rfc/rfc5322.txt)
的空行分离和 [Jekyll front matter](https://jekyllrb.com/docs/front-matter/)
的显式起止定界：共同原则是先结束头部，再将剩余字节视为正文。本项目只借鉴确定的
framing，不引入邮件、HTTP 或 YAML 协议。两个主源已实读并留存 SHA。

读取旧发布 v1 时只解析 marker，保留其 Boundary 身份与全部正文；不迁移未发布的
Tail-v1 候选。新 Tail 的同版本重入和 checkpoint 恢复仍按 v2 元数据去重。
exact marker 的查询、替换与删除通过同一中立 parser 支持 v1/v2；旧 Boundary 首次
重新物化为 v2 可产生一次正常 generation 冷 miss，不把该格式切换声称为历史全字节稳定。

公开结构体字面构造需改用 `ContextProjection::replacing` 或 `versioned`；默认 placement 保留边界替换含义。示例 `context_projection_cache` 随 examples 构建。SDK 的精确 pin 独立，不自动更新；工具配对、原厂 ID、签名、权限、发布和 usage 口径均沿原权威。

消费者盘点确认 SDK Host 的 `ExtensionAgentComponentProxy` 仍按其独立精确 revision `340993201cc937fc8e4783b66be399ef9f14e3d5` 构造旧二字段投影。本次不修改该 pin 或已接受的扩展 wire；将来升级 framework pin 时须同步适配构造入口，并单独决定是否向跨语言合同开放版本语义。本地框架与 Eclip 验证不构成该 SDK 新 revision 资格。官网当前没有依赖这份构造 API 的缓存说明，不新增命中率量化宣传。

固定规则仍可能使前缀冷启动。内部合同及真实 adapter 输出稳定只能证明没有内部前置重写；真实命中收益仍需相同 vendor/endpoint/protocol/model 下的连续增长 workload 对照，不能承诺固定 95%。
