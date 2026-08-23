# 面向 echo-agent 贡献者的 Rust 学习指南

这套教程面向没有系统学过 Rust、但需要阅读和修改 `echo-agent` 的开发者。它不是把 Rust 语法做成
一张速查表，而是沿本项目真实代码逐步回答：值由谁拥有、失败怎样返回、动态实现怎样组合、异步任务
怎样共享与取消，以及改完代码怎样证明它可靠。

课程由 **15 篇文档 + 1 个可运行教学 crate + 13 个示例 + 单元/集成测试**组成。配套代码位于
[`echo-rust-learning`](../../README.md)，只依赖框架公开 API，不参与任何生产
运行路径，也不需要模型、API Key 或网络。

## 零基础从这里开始

在仓库根目录执行：

```bash
rustup show active-toolchain
cargo test -p echo_rust_learning
cargo run -p echo_rust_learning --example chapter_01_basics
```

根目录 [`rust-toolchain.toml`](../../../rust-toolchain.toml) 选择 stable 工具链并安装 rustfmt、Clippy；
教学 crate 的最低 Rust 版本声明为 1.95，edition 为 2024。命令失败时先读第一条错误，不要跳过失败
继续学习后续章节。

推荐一边打开文档，一边在编辑器中打开 `examples` 目录下对应章节的示例文件。每章的完整实现都在
`src/` 模块中，示例负责把几个概念串成可观察行为，测试负责固定边界和错误路径。

## 完整学习路线

| 阶段 | 章节 | 配套代码 | 学完后能做什么 |
|------|------|----------|----------------|
| 工程入口 | [01 环境、Cargo、workspace 与模块](01-cargo-workspace-modules.md) | crate 目录结构 | 找到 package/crate/module 边界并运行正确 target |
| 语言基础 | [02 变量、类型、表达式与控制流](02-language-basics.md) | `chapter_01_basics` | 阅读绑定、数值、切片、函数、if/loop/match |
| 领域建模 | [03 struct、enum、方法与模式匹配](03-domain-modeling-pattern-matching.md) | `chapter_03_domain_modeling` | 用类型表达状态和不变量 |
| 核心门槛 | [04 所有权、借用与生命周期](04-ownership-borrowing-lifetimes.md) | `chapter_04_ownership` | 判断 move、borrow、clone 与引用有效范围 |
| 数据处理 | [05 集合、闭包与迭代器](05-collections-closures-iterators.md) | `chapter_05_collections_iterators` | 使用 Vec/Map/Set 和所有权明确的迭代链 |
| 可靠性 | [06 Option、Result 与错误处理](06-option-result-errors.md) | `chapter_06_errors` | 用 `?`、错误链和领域错误替代 panic |
| 抽象能力 | [07 trait、泛型、宏与 feature](07-traits-generics-macros.md) | `chapter_07_traits_generics` | 读懂 trait object、Builder、`#[tool]` 和条件编译 |
| 智能指针基础 | [08 Box、Deref、Drop 与 RAII](08-smart-pointers-foundations.md) | `chapter_08_box` | 理解堆分配、递归类型、动态值和资源释放 |
| 共享所有权 | [09 Rc、Arc 与 Weak](09-shared-ownership.md) | `chapter_09_arc_weak` | 设计强弱引用方向并理解 Send/Sync 传播 |
| 共享修改 | [10 Cell、RefCell、锁与原子类型](10-interior-mutability.md) | `chapter_10_rc_refcell` | 选择运行时借用、锁、原子或 channel |
| 异步底层 | [11 Cow、Pin、Future 与 Stream](11-cow-pin-futures.md) | `chapter_11_pin_future` | 拆解 BoxFuture、动态 Stream 和异步生命周期 |
| 异步运行时 | [12 Tokio、channel、超时与取消](12-async-concurrency-streams.md) | `chapter_12_async_concurrency` | 组织任务、背压、结构化并发和取消 |
| 数据边界 | [13 Serde、JSON、配置与协议](13-serde-and-configuration.md) | `chapter_13_serde` | 区分解析、强类型转换、验证和 wire contract |
| 工程质量 | [14 测试、rustdoc、fmt 与 Clippy](14-testing-and-tooling.md) | 单元测试、`learning_contract` | 写确定性测试并读懂编译器诊断 |
| 项目实战 | [15 阅读和修改 echo-agent](15-reading-echo-agent.md) | `chapter_15_echo_agent_tool` | 沿真实工具路径完成第一次框架改动 |

新手应按顺序完成 01-08，这部分建立 Rust 最关键的所有权和错误处理基础。09-12 是本项目异步共享
架构的核心，不能只记住类型名。已有 Rust 经验的读者可直接运行各章示例自测，再重点阅读项目映射。

## 三条短路线

时间有限时仍应先读第 04、06 章，再按任务选择：

- **修改普通领域代码**：01 → 03 → 04 → 05 → 06 → 07 → 14 → 15。
- **修改 Agent 异步运行时**：04 → 06 → 08 → 09 → 10 → 11 → 12 → 14 → 15。
- **修改配置、事件或工具协议**：03 → 06 → 07 → 11 → 13 → 14 → 15。

短路线用于已有基础者定位，不代表被跳过的章节不重要。

## 每章怎么学

1. 先读“解决什么问题”，不要直接背 API。
2. 运行章节示例，修改一个输入并预测输出。
3. 打开 `src/` 中完整实现，区分公共签名与内部细节。
4. 运行该模块测试，观察成功、错误和边界路径。
5. 打开“项目映射”的生产源码，标出同一种 Rust 模式。
6. 完成至少一个练习，再运行 `cargo test -p echo_rust_learning`。

遇到编译错误时，先判断它属于类型不匹配、所有权/生命周期、Send/Sync 还是 feature/可见性问题。
不要为了快速通过编译而到处加 clone、Arc 或 `'static`。

## 代码地图

| 模块 | 主要知识 |
|------|----------|
| [`fundamentals.rs`](../../src/fundamentals.rs) | 基础类型、切片、控制流、模式匹配 |
| [`basics.rs`](../../src/basics.rs) | 领域结构、方法、UTF-8 安全处理 |
| [`ownership.rs`](../../src/ownership.rs) | move、borrow、生命周期与 newtype |
| [`collections.rs`](../../src/collections.rs) | Vec、HashMap、HashSet、闭包、迭代器 |
| [`errors.rs`](../../src/errors.rs) | Option、Result、thiserror 与上下文 |
| [`traits.rs`](../../src/traits.rs) | trait、泛型、动态分发与 Builder |
| [`smart_pointers`](../../src/smart_pointers/mod.rs) | Box、Rc、Arc、Weak、RefCell、锁、Cow、Pin |
| [`async_concurrency.rs`](../../src/async_concurrency.rs) | Tokio 任务、channel、超时与取消 |
| [`serialization.rs`](../../src/serialization.rs) | Serde 与验证边界 |
| [`project_patterns.rs`](../../src/project_patterns.rs) | 真实 echo-agent `#[tool]` API |

## 术语先不要混淆

- **所有者**负责值何时被销毁；借用者只在允许范围内临时访问。
- **move** 转移所有权，不等于把堆数据逐字节复制一遍。
- **clone** 按类型定义产生新值；`Arc::clone` 只增加引用计数。
- **智能指针**是带额外所有权或访问语义的类型，不代表自动垃圾回收。
- **并发**是任务交错推进；**并行**是多个核心同时计算。
- **Future** 描述尚未完成的计算；调用 async 函数不会自动创建线程。
- **trait object** 用动态分发统一异构实现；泛型默认使用静态分发。

## 课程代码约束

- 所有完整示例离线、确定性运行。
- 中文或 emoji 字符串使用 `chars()`，不做 UTF-8 字节截断。
- 不使用 `unwrap`、`expect`、`panic!`、`todo!`、危险直接索引或未处理解析。
- 整数累计使用 `checked_*` 或 `saturating_*` 明确溢出策略。
- 取消、超时和业务失败使用不同错误语义。
- 只使用 Subagent 术语，不引入第二套执行角色命名。
- 修改框架前必须阅读根目录 `AGENTS.md`，项目规则优先于通用教程。

完成课程不等于掌握 Rust 的全部内容，但应足以让你在编译器、测试和代码审查的帮助下，安全阅读并
修改本项目使用到的 Rust 代码。
