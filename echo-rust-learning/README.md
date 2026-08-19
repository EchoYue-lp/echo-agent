# echo-rust-learning

`echo_rust_learning` 是 `echo-agent` workspace 内的非发布教学 crate。它用可运行、可测试的代码解释
贡献者阅读框架源码时会遇到的 Rust 概念，不参与生产运行路径。生产 crate 不依赖本 crate；教学
crate 单向依赖 `echo_agent` 的公开 API。

完整的 15 章中文课程从[学习指南首页](../docs/zh/rust-learning/README.md)开始。

## 运行

在 `echo-agent` 仓库根目录执行：

```bash
cargo test -p echo_rust_learning
cargo run -p echo_rust_learning --example chapter_01_basics
cargo run -p echo_rust_learning --example chapter_12_async_concurrency
cargo run -p echo_rust_learning --example chapter_15_echo_agent_tool
```

所有示例均离线运行，不需要模型、API Key 或外部服务。`examples/` 展示完整使用路径，`src/` 保存
可复用教学实现，`tests/learning_contract.rs` 验证跨模块公共契约。

## 示例索引

| 示例 | 对应章节 | 内容 |
|------|----------|------|
| [chapter_01_basics](examples/chapter_01_basics.rs) | 02 | 变量、切片、控制流、UTF-8 |
| [chapter_03_domain_modeling](examples/chapter_03_domain_modeling.rs) | 03 | struct、enum、方法、模式匹配 |
| [chapter_04_ownership](examples/chapter_04_ownership.rs) | 04 | move、借用、生命周期、newtype |
| [chapter_05_collections_iterators](examples/chapter_05_collections_iterators.rs) | 05 | Vec、Map、Set、闭包和迭代器 |
| [chapter_06_errors](examples/chapter_06_errors.rs) | 06 | Option、Result、错误链与 `?` |
| [chapter_07_traits_generics](examples/chapter_07_traits_generics.rs) | 07 | trait、泛型、动态分发和 Builder |
| [chapter_08_box](examples/chapter_08_box.rs) | 08 | Box、递归类型、Deref 与 Drop |
| [chapter_09_arc_weak](examples/chapter_09_arc_weak.rs) | 09 | Arc、Weak、强弱计数和注册表 |
| [chapter_10_rc_refcell](examples/chapter_10_rc_refcell.rs) | 10 | Rc、RefCell 与运行时借用冲突 |
| [chapter_11_pin_future](examples/chapter_11_pin_future.rs) | 11 | Cow、Pin、Future 与 BoxFuture |
| [chapter_12_async_concurrency](examples/chapter_12_async_concurrency.rs) | 12 | Tokio、channel、锁、超时和取消 |
| [chapter_13_serde](examples/chapter_13_serde.rs) | 13 | Serde、JSON round-trip 和验证 |
| [chapter_15_echo_agent_tool](examples/chapter_15_echo_agent_tool.rs) | 15 | 离线执行真实 `#[tool]` |

第 01 章讲 Cargo 工程，第 14 章讲测试工具，因此没有单独 example；它们直接使用整个 crate 和
测试套件作为练习对象。

## 模块索引

```text
fundamentals           基础类型与控制流
basics                领域结构与 UTF-8
ownership             所有权、借用与生命周期
collections           集合、闭包与迭代器
errors                Option、Result 与结构化错误
traits                trait、泛型和 Builder
smart_pointers        Box/Rc/Arc/Weak/RefCell/Cow/Pin/锁/原子
async_concurrency     Tokio 任务、channel、超时与取消
serialization         Serde 与配置验证
project_patterns      echo-agent 真实 Tool API
```

## 建议的修改循环

```bash
cargo check -p echo_rust_learning --all-targets
cargo test -p echo_rust_learning
cargo fmt --all -- --check
cargo clippy -p echo_rust_learning --all-targets -- -D warnings
```

教学代码同样遵守仓库的无 panic、UTF-8 安全和结构化错误规则。练习时应通过 `Result`、`Option` 和
可恢复 API 解决失败，不用强制提取绕开编译器。
