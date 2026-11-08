# LSP退役与历史恢复

## 当前源码边界

LSP 作为具名能力从当前 Framework 源码撤销：不再提供 `lsp` Cargo feature、LSP client／manager、
语言服务进程与五个 LSP 工具，也不保留返回空成功的兼容控制器。应用不能通过旧配置或插件声明
重新启动它。其它通用 `Tool`、ACP、MCP、channels、Subagent 和存储实现不因此删除。

旧项目 `.lsp.yaml` 和插件 `lsp.yaml` 都是用户数据，保持原样。以下冻结与诊断只针对
原插件 prepare 识别的插件根 `lsp.yaml`，不宣称 Framework 扫描项目级 `.lsp.yaml`。
该插件声明沿原 registry／prepare 的通用 diagnostics 报告退役；合法文本由同一 generation 保持完整冻结内容和 identity，
IO、非法 UTF-8、非普通文件及坏链接保留原错误，不能被退役 Warning 覆盖。缓存命中不重读，
显式 invalidation 才反映源变动。健康 Skill、Hook、MCP、Subagent 与原 publication／rollback／
shutdown owner 继续处理，不新增 reader、executor 或第二状态权威。

## 保留的编码与桌面能力

项目读取、代码搜索、Shell、Git、工具扩展及 Subagent 仍由各自能力与权限合同提供。
EKO GUI 内置 PTY／SSH 终端、Agent shell 和独立 SDK Host stdio 不属于 LSP 退役范围；
这些能力并不承诺模拟语言服务结果。需要自定义工具时仍沿通用 Tool／MCP 接口，不能把一个
同名工具当成已退役 LSP 的内置控制面。

## 历史源码与恢复

本机固定 `pre-retirement-2026-10-24` tag 和 `chore/Echoyue/full-capabilities-baseline`
分支保留完整旧实现。先只读对照，例如：

```bash
git show pre-retirement-2026-10-24:src/lsp.rs
git show pre-retirement-2026-10-24:src/tools/lsp.rs
```

如需恢复，另开明确授权的任务评估向当前主线移植哪些合同；不要 reset 当前工作区、改指固定
refs 或覆盖现有数据。旧 ADR 与测试原件记录其历史切点，不表示当前主线仍有该 feature。

## 验证资格

源码删除、消费者去依赖、删后构建和锁、正式 SDK Git pin、原生宿主验收及发布分别结算。
文档更新、旧 focused 通过或受控 Host 均不能替代后继真实源的适用门禁。

详见[ADR0090](../adr/0090-a2a-lsp-retirement-contract.md)、[工具系统](./02-tools.md)、
[MCP](./08-mcp.md)与[插件系统](./32-plugin-system.md)。
