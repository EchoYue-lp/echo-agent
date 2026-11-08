# ADR 0110：在原 SearchProvider 中接入 Exa

状态：原独有分支功能已移植至集成候选；新切点验证、main 合流和官网同步分别验收。

## 背景与候选方案

`web_search` 搜索互联网并返回标题、URL 和摘要；`web_fetch` 读取已选 URL。框架已有通用
`SearchProvider` 与共享 limit/cursor 分页，Exa 不是应用专属策略，不另建搜索工具、trait、
应用搜索循环或权限门控。原 `feature/Echoyue/exa-web-search` 的 `4343f0b7` 含独有实现，
当前集成底座还没有对应 provider，不能以其它后继功能为由丢弃。

候选包括新增独立 Exa 工具、合并搜索/网页获取、只新增构造器但不接标准包。前两种重复
已有职责，第三种无法让标准包自动选择 Exa。本次复用原 trait 并接通标准包。

## 决策与参考

新增 `providers::exa::ExaSearchProvider`、`WebSearchTool::with_exa(key)`。参考原开发时的
[Exa 官方搜索接口](https://docs.exa.ai/reference/search)与
[LangChain Exa 集成](https://docs.langchain.com/oss/python/integrations/tools/exa_search)：
将厂商传输留在 provider 边界，使用 `POST https://api.exa.ai/search`、Bearer Key、
`type: auto`、有界 `numResults`，只请求 highlights 并归一化为 `snippet`；不调用 contents
接口，不改变独立 `web_fetch`。本次官方文档联网复核未成功，未据此宣称新版本线上验证。

`auto()` 在构造时按非空 `EXA_API_KEY` > 原 Tavily > Brave > DuckDuckGo 选择；标准包改用
`auto()`。无 Exa Key 时沿原三家顺序，显式构造器仍可固定引擎；请求失败不自动换引擎。
修改环境后须重建工具/Agent。该选择不改变任务重试、权限、模型或发布权威。

原分支错误文字包装已被当前底座取代，因此只移植功能，错误接入现有 `http_body`：
传输走 `request_error`，非成功响应走 `response_error`，成功 JSON 走原有界读取。
`RemoteFailure`、`ExecutionNotSettled`、HTTP status 和 Retry-After 保持当前类型权威，
不恢复统一 transient 或 `ExecutionFailed` 包裹，不解析展示文字决策重试。
401/402永久失败，429/5xx沿原瞬态规则，已有超时/取消/不完整执行语义不变。

本地场景也必须防止密钥与查询泄漏：失败不读上游响应正文，传输诊断复用原 URL 清理；
不把 headers 或原始正文写进日志。未新增依赖、锁、feature、配置凭据或持久化 store。

## 影响与验证边界

保留原五个 provider 用例：实际本机 HTTP 请求/Unicode highlights、空与损坏结果、
分页、错误隐私、数量边界；错误用例增加 typed分类、Retry-After 与工具层 metadata。
选择测试在独立子进程设置环境，不改父测试全局环境；以非法合成 header 在网络前失败，
证明标准包安装了 Exa 且未静默换引擎。完整 `web` feature 与 demo41 示例编译须由统一
构建窗口在新切点实际执行，原分支历史绿色不能代替本次结果。

`web_search` 仍取最多十条结果供本地 limit/cursor 分页；provider直接调用最多100条、0条
不发请求。中文正式说明、README 和示例 env 同步；旧 ADR0090 已用于 A2A/LSP 退役，
本次使用0110，不改退役合同、不新建英文镜像。官网在干净框架提交后使用原同步机制处理，
SDK Host 显式 DuckDuckGo 与独立 pin 保持原状。无具名授权不调用真实 Exa 服务。
