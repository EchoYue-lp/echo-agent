---
title: ChannelManager owned operations
artifact: design
carrier: markdown
---

# ChannelManager owned operations

## Status

accepted

本设计属于用户已授权的SDK全能力治理修复。只形成一个Outcome：释放注册表锁后调用原channel实例，保留框架唯一生命周期权威。新增Rust机制不自动接受为三语言API。

## 问题与目标

真实ChannelHealth回调等待同manager.ids，SDK持manager Mutex等待health，形成循环等待。原日志/tmp/sdk-channel-manager-read-reentry.log。1.5秒屏障仅证明顺序，不能用超时作为修复。start/send/stop/close同样不得持全局manager锁等待外部回调。

## 目标行为

取得owned操作后释放manager guard再await；health/send/start/stop/handler.close回调能够等待同manager ids/list。不同plugin不共享覆盖异步操作的全局锁。失败或取消保留原handler与transport清理责任，stop成功不等于unregister。

## 范围与非目标

包括framework ChannelManager、公共借用/owned访问与生命周期、SDK现有channel适配和close、文档及验证。不新增receive/inbound wire、权限策略、调度器、持久化、重连、SDK lookup map或以名称重建proxy；不宣称同plugin回调递归等待自身mutation也可安全完成。

## 分层与复用

framework拥有原plugin owned reference、entry生命周期串行化、handler清理、transport stop事实及批量快照；SDK只校验既有handle、短锁取得owned操作、释放锁、await和转换。

全仓已查：ChannelManager现在Boxmap/get借用，没有owned入口；ToolManager::get_tool和McpManager::get_client/get_clients已有返回原Arc的模式。标准Arc和现tokio锁足够，不加依赖。旧handler map/stopped set语义迁入同entry，不能长期双写或留下平行rollback循环。

## 参考实现与取舍

[Tokio共享状态](https://tokio.rs/tokio/tutorial/shared-state)将短状态访问与异步工作分开；[Tokio RwLock](https://docs.rs/tokio/latest/tokio/sync/struct.RwLock.html)说明公平锁仍可能嵌套等待死锁，换RwLock本身不是重入保证。[MCP cancellation](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/cancellation)是请求级通知，不能代替真实关闭结算。

拒绝SDK第二map/重建proxy（身份与owner分裂）、仅修health（其它调用仍锁等待）、新actor（无需另执行模型）。采用framework同一owned entry与owned operations，保持既有调用语义。

## 设计

采用框架唯一注册表中的owned entry；适配器释放manager锁后await原实例操作，生命周期结算继续归同一entry。

## 核心结构与数据流

ChannelManager唯一注册表保存ChannelHandle→Arc<ChannelEntry>。entry包含原Box<dynChannelPlugin>、原handler责任、transport-stopped事实。entry锁串行化&mut生命周期；ids/list读注册表不等entry锁。id/label/capabilities明确为注册时声明，注册生命周期内固定；查询不等待plugin锁。全仓Qq/Feishu为常量或staticOnceLock，SDK proxy为不可变descriptor字段，CLI/probe无动态metadata消费者。旧trait理论允许动态label/capabilities，这一边界变化必须写入ADR与public文档：第三方需要更新声明时使用新的注册生命周期，不能期待通过start/stop隐式刷新。保留get/get_mut签名与原插件异步能力，不宣称同步引用可动态读取异步锁内数据。不引入unsafe、同步阻塞锁或owned存在时get_mut突然None。

新增get_owned(id)、start_all_owned(factory)、stop_owned(id)、stop_all_owned返回不借manager的owned future。现get/get_mut保留同wrapper trait访问，get_mut必须保“下次managed stop重新调用transport”语义。同步get_mut以mutation epoch使旧stopped事实失效；epoch饱和时保守调用transport stop，不允许溢出或盖过并发的新失效。原start_all/stop/stop_all委托同entry唯一实现。

owned引用保留原实例/generation，不按name再查。批量同步构造时捕获当时entries；随后新增注册不入本次，下次可见。未poll即drop不产生副作用；poll后产生责任留原entry。批量每结果携id，单项失败不妨碍其它结果。

SDK所有metadata/health/send/start/stop_exact/stop_all以及Session/resource close都使用短manager guard取操作，然后锁外await。不得在SDK复制批量loop/rollback或只修health。

## 生命周期与异常

Start先检查未结算handler，首次await前保存handler责任，失败执行原plugin.stop/handler.close rollback。失败或取消保留责任，不能被下一start覆盖。

Stop同entry串行，transport未成功停止时调用stop，成功后才置stopped，再close原handler；handler失败/取消保留，重试不重复停止已停transport。成功close才清handler。并发stop共享同authority，ownedArc存活不表示已关闭。

Manager Drop继续报告未结算责任；显式awaited shutdown才是关闭入口。SDK现retained close owner保失败、成功后才释放handle，不能另造close状态机。

同plugin递归等待同entry锁不在本次保证中，不用递归锁或cache假保证；若真实accepted路径必须支持，单独记录并明确处理。不同plugin操作不被全注册表锁阻塞。

## 验收标准

- 原health→ids反例在callback返回前完成ids；health/send/start/stop/handler.close各回调重入metadata真实成功。
- A回调停驻时B metadata/独立操作可达；同entry start/stop串行。
- start失败且rollback不全、start取消后stop均保原handler责任并可重试。
- stop成功/handler.close失败或取消后，只重试handler；get_mut后再次stop真实调用transport。
- 同步batch后注册不入当前batch、下次入；未poll的future drop无副作用。
- registration metadata固定声明、现native/proxy相同值；第三方动态元数据兼容变化显式可审查。持entry异步锁期间ids/list仍可读。
- Session/resource close失败保owner、成功后旧handle失败/配额复用；同名其它manager不受影响。
- 三语言真实Host共用metadata reentry场景，比较callback顺序、结果、错误；没有fakeinbound或新wire。
- 全仓查无旧双生命周期权威；focused/完整门禁、示例、独立复审、公开pin/main逐快照验收。

## 文档与交付

正式framework文档及ADR记录owned机制；SDK说明释放外锁和metadata重入边界。原examples仍可执行验证，官网从clean published source同步。最后SDK pin必须已验证已推送successor，localpatch只候选证据。
