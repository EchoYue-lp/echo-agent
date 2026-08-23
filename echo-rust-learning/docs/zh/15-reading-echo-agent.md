# 15 阅读和修改 echo-agent 源码

前面的语言知识最终要落到真实框架。面对大型 Rust workspace，不要从最长的执行循环开始逐行读。
先找公共契约，再沿一条可运行路径进入实现；每到一个类型，都回答它属于哪层、谁拥有它、错误和
事件如何返回。

## 先建立 workspace 地图

```text
echo_core            通用 trait、事件、错误、消息和工具契约
     ↓
echo_state           存储、压缩、审计
echo_execution       工具执行、沙箱、Skill
echo_integration     模型、MCP、channel 适配
echo_orchestration   工作流、任务、人工介入
     ↓
echo_agent           facade、ReAct Agent、运行管线

echo_rust_learning   只消费公开 API，不被生产 crate 依赖
```

箭头表达允许的依赖方向，不是运行时调用的全部细节。`echo-agent` 是通用框架，具体 EKO 产品策略
属于另一个应用仓库；不能因为某个字段“可以配置”就把应用决策塞进框架。

## 准备源码导航工具

```bash
# 按文件名浏览
rg --files | rg 'agent|tools|state'

# 搜定义
rg -n 'pub trait Tool|struct ReactAgentBuilder'

# 搜构造与调用
rg -n 'ToolManager::new|\.execute\('

# 查看 package 和 target 图
cargo metadata --no-deps
```

rust-analyzer 的“跳转到定义”“查找引用”“显示类型”非常适合宏和泛型代码。`cargo doc --open` 则
按公共 API 和 trait 实现组织内容，常比目录树更快说明一个类型能做什么。

搜索时区分三件事：类型已经定义、能力已经注册、主运行路径实际可达。一个 `pub` 类型可能是合理
框架扩展点，即使本仓库示例没有构造；一个已注册工具也可能被永远为 false 的条件挡住。不能只凭
一次文本命中判断它是活代码或死代码。

## 第一步：公共 facade

先看 [`src/lib.rs`](../../../src/lib.rs)：

1. 根 crate 声明哪些 module。
2. 哪些类型通过 `pub use` 重导出。
3. prelude 为用户提供哪些最常用入口。
4. 哪些入口受 feature 控制。

这一步回答“下游用户真正能依赖什么”。内部文件存在不等于它属于公开契约。

## 第二步：四个核心 trait/类型

阅读：

- [`Tool`](../../../echo-core/src/tools/mod.rs)
- [`Agent` 与 AgentEvent](../../../echo-core/src/agent/mod.rs)
- [`LlmClient`](../../../echo-core/src/llm/mod.rs)
- [`ReactError`](../../../echo-core/src/error.rs)

不要立即钻进实现，先抄下每个签名并逐层拆解：

- 参数由谁拥有，哪些只是借用？
- 返回 Future 为什么需要某个生命周期？
- trait 是否要求 Send + Sync？
- 返回的是单值、Result 还是 Stream？
- 错误是否保留 source，取消是否有独立语义？

看懂这些契约后，复杂实现只是满足契约的不同方式。

## 第三步：从最小示例走一条垂直路径

从 [`demo00_quickstart.rs`](../../../examples/demo00_quickstart.rs) 开始：

```text
示例创建配置
  -> ReactAgentBuilder 收集依赖并校验
  -> build 产生 Agent
  -> execute/stream 进入 ReAct 运行管线
  -> LlmClient 产生模型响应
  -> ToolManager 查找并执行工具
  -> AgentEvent/Result 返回调用方
```

每跳一步就用“查找引用”确认真实调用点，记录具体类型从哪里被擦除为 `dyn Trait`、从哪里包入 Arc、
从哪里进入 async task。这样所有权和分层会形成一张图，而不是一组孤立文件。

## 第四步：跟踪一个 Tool

教学 crate 的 [`chapter_15_echo_agent_tool.rs`](../../examples/chapter_15_echo_agent_tool.rs)
离线执行真实 `#[tool]`。先运行：

```bash
cargo run -p echo_rust_learning --example chapter_15_echo_agent_tool
```

然后依次回答：

1. `#[tool]` 宏为普通 async 函数生成了哪些 trait 实现？
2. JSON 参数在哪个边界转换成 Rust 参数？
3. 函数错误如何进入框架统一错误类型？
4. 返回值如何变成 ToolResult？
5. ToolManager 用独占 Box 还是共享 Arc 保存它，为什么？

这条路径会串起宏、Serde、trait object、BoxFuture、错误转换和测试，是本教程的综合练习。

## 第五步：跟踪一条状态能力

可选择 Store 做垂直阅读：

1. 在 `echo_core` 找 Store trait 和数据类型。
2. 在 `echo_state` 找 InMemoryStore、FileStore 等实现。
3. 在根 `echo_agent` 找重导出。
4. 在 Builder 找 `Arc<dyn Store>` 如何注入。
5. 在运行管线或工具中找真实调用。
6. 检查不同实现是否有共享契约测试。

框架提供多种合理实现是能力菜单。不能因为某个应用当前没使用某一公开实现，就断定框架应删除它。

## 语言模式速查

| 看到的代码 | 立即追问 |
|------------|----------|
| `Option<T>` | 缺失是否合法，None 在哪层处理？ |
| `Result<T, E>` + `?` | 当前层补了什么上下文，source 是否保留？ |
| `Box<dyn Trait>` | 为什么需要异构和动态分发，谁独占它？ |
| `Arc<dyn Trait>` | 哪些任务共享，T 是否 Send + Sync？ |
| `Weak<T>` | 谁是真正所有者，upgrade 失败如何处理？ |
| `Arc<RwLock<T>>` | 锁保护哪条不变量，guard 是否跨 await？ |
| `BoxFuture<'a, T>` | Future 借用了谁，为什么需要类型擦除？ |
| `Pin<Box<dyn Stream>>` | 流如何终止、取消和报告错误？ |
| `#[cfg(feature = "...")]` | 哪些组合真正编译过？ |
| `#[derive(...)]` / `#[tool]` | 宏生成了哪些 impl 和 wire contract？ |
| `impl Into<String>` | API 在边界何时取得所有权？ |

## 修改前的强制门禁

新增字段、trait、store、validator 或执行机制前：

1. 在整个仓库按名字、行为和相邻概念搜索，确认是否已有实现。
2. 区分定义、注册和主路径可达性。
3. 写下“通用机制 / 产品策略 / 适配边界”，明确应落在哪个 crate。
4. 同一语义只能有一个权威实现；迁移后删除被替代路径。
5. adapter 只做无损类型转换与策略注入，不重新实现通用调度。
6. 公共 trait、feature 或依赖变化后执行 workspace 与独立 feature 验证。

遇到架构、状态机、Agent 编排或 API 形状等关键决策，还必须先调研 Claude Code、Codex 等成熟
系统的官方资料和实现共性，再结合本项目定位取舍。小型 bug 和文案修改不需要把流程扩大化。

## 一个适合新手的修改循环

```text
复现或写失败测试
  -> 找最小权威实现
  -> 写范围最小的修改
  -> 跑相关 crate 快检
  -> 阅读并修复全部 warning/error
  -> 跑完整适用门禁
  -> 检查 git diff 是否只包含预期内容
```

第一次贡献可从纯函数 Unicode 边界、错误上下文、文档偏差或 trait 多实现一致性测试开始。改动小不
代表质量要求低；编译器、测试和 Clippy 正是让新贡献者安全修改复杂系统的护栏。

## 代码审查自检

- 字符串截断是否使用 `chars()`，没有字节索引？
- 外部输入是否都通过 Result/Option 处理，没有 panic API？
- clone 是为了短暂取得所有权，还是掩盖了不清晰的所有者？
- Arc、锁和 channel 各自解决的问题是否明确？
- 锁 guard 是否在 `.await` 前释放？
- 取消、超时、业务错误是否保持结构化区别？
- 新类型是否已有同义实现，是否放在正确分层？
- 测试是否覆盖错误路径、Unicode 和 feature 边界？
- 文档示例、链接、命令是否真实可运行？

## 最终实践

1. 运行 `chapter_15_echo_agent_tool` 并阅读其测试。
2. 给 greeting 工具增加一个可选、强类型参数，并更新 JSON 参数测试。
3. 沿宏生成入口解释参数如何从 JSON 变成 Rust 值。
4. 运行教学 crate 全部 example、test、fmt 和 Clippy。
5. 从快速上手示例画出一条真实 Tool 调用路径，标出每次 move、borrow、Arc clone 与错误转换。

完成后继续阅读项目的[快速上手](../../../docs/zh/getting-started.md)和[工具系统](../../../docs/zh/02-tools.md)，将这套阅读方法
用于真实 Agent 功能。
