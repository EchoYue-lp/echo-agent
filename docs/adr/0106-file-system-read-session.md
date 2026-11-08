# ADR 0106：文件提供者的可选只读会话

日期：2026-11-06。

## 背景与候选

远程文件提供者若为每次 stat/read/list 重新建立子通道，通用加载算法会放大连接开销。永久连接池会扩大退出、重连和写入语义；由产品按文件提供者类型向下转换则破坏已有抽象。

采用 `FileSystem::begin_read_session`，默认返回 `None`；支持批次复用的提供者返回 `FileSystemReadSession`，暴露当前目标内的 `files()`，并通过可失败的 `finish` 结算。它表达有界只读传输借用，不承诺事务快照、不缓存跨调用内容、不含任何 SSH 或 Eclip 产品字段。

## 契约与取舍

调用方在原 blocking 边界使用，会话只服务当前读取阶段。实现必须禁止副作用和资源逃逸，并在 Drop 中结算原资源；调用方正常路径显式 finish 以观察关闭错误。丢弃 observer 不能被解释为物理操作取消。现有 FileSystem 实现无需改动即可保持原行为，普通写入 API、错误分类和原资源 owner 不变。`first_existing_path` 默认串行实现；远程提供者可以有界流水化 metadata 并按输入顺序解释结果，不能通过完整目录枚举放大成本，也不能吞掉优先路径的错误。

[openssh-sftp-client](https://docs.rs/openssh-sftp-client/0.15.10/openssh_sftp_client/)支持客户端内请求缓冲，也明确 future 取消不撤回远端操作；[TRAMP](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/tramp.texi)复用连接信息并规定失效。这些实现支持有明确生命周期的复用，不支持把取消当回滚。

## 影响与验收

新增默认只读会话和有序 `first_existing_path` trait 方法及一个对象安全的会话 trait，没有依赖变更。编译文档展示使用方式；Eclip 消费者验证一次准备复用、多消息刷新、失败时关闭和真实本地 SFTP 协议。SDK Protocol 和 Host 合同未变化；没有删除现有公共能力。
