# 01 开发环境、Cargo、workspace 与模块

Rust 编译器只负责把源码编译成程序，Cargo 则负责依赖、构建、测试、文档和条件编译。
第一次进入本项目时，先学会回答两个问题：一段代码属于哪个 crate，以及应该用哪条 Cargo
命令验证它。否则很容易在错误的层级加代码，或只验证了 workspace 的一小部分。

## 安装与确认工具链

Rust 官方工具链管理器是 `rustup`。本仓库通过
[`rust-toolchain.toml`](../../../rust-toolchain.toml) 固定 Rust 版本和组件；进入目录后，
`cargo` 会自动选择该工具链。

```bash
rustup show active-toolchain
rustc --version
cargo --version
```

- `rustc` 是编译器。
- `cargo` 是构建与包管理工具。
- `rustfmt` 统一代码格式。
- `clippy` 提供比编译器 warning 更丰富的静态检查。
- `rust-analyzer` 为编辑器提供跳转、补全和实时诊断。

编译器错误通常比其他语言更长。先看第一条 `error[...]`、源码箭头和最后的 `help`，后面的错误
经常只是第一条错误的连锁反应。

## package、crate、module、workspace

这四个词位于不同层级：

| 概念 | 它是什么 | 本项目示例 |
|------|----------|------------|
| package | 一个 `Cargo.toml` 描述的构建/发布单元 | `echo-rust-learning` |
| crate | 一次编译得到的库或二进制 | `echo_rust_learning` 库 crate |
| module | crate 内的命名空间与可见性边界 | `smart_pointers::cow` |
| workspace | 共享 `Cargo.lock` 和构建目录的一组 package | 仓库根 workspace |

一个 package 最多有一个库 crate，但可以有多个二进制、示例和测试 crate。默认文件约定是：

```text
src/lib.rs          库 crate 入口
src/main.rs         默认二进制入口
src/bin/*.rs        额外二进制
examples/*.rs       可运行示例，每个文件都是独立 crate
tests/*.rs          集成测试，每个文件也是独立 crate
benches/*.rs        基准测试
```

package 名可以有连字符，Rust 路径中的 crate 名会把连字符转为下划线。例如 Cargo 参数使用
package 名 `echo-rust-learning`，源码写 `use echo_rust_learning::...`。

## 读懂 Cargo.toml

教学 crate 的 [`Cargo.toml`](../../Cargo.toml) 包含三类关键信息：

```toml
[package]
name = "echo-rust-learning"
publish = false

[dependencies]
echo_agent = { path = ".." }
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

- `[package]` 描述名字、版本、edition 和是否发布。
- `[dependencies]` 是运行库代码需要的依赖。
- `[dev-dependencies]` 只供测试、示例和基准使用。
- `path = ".."` 依赖本地 package；`workspace = true` 复用根 manifest 统一声明的版本。

`Cargo.lock` 记录完整依赖解析结果。应用和本仓库会提交它，让 CI 与本地使用相同版本；不要手工
修改 lockfile，应由 Cargo 更新。

## 最常用的 Cargo 命令

```bash
# 快速做类型检查，不生成最终可执行文件
cargo check -p echo_rust_learning --all-targets

# 运行教学 crate 全部测试
cargo test -p echo_rust_learning

# 运行一个示例
cargo run -p echo_rust_learning --example chapter_01_basics

# 只运行名称中含 ownership 的测试
cargo test -p echo_rust_learning ownership

# 生成 API 文档
cargo doc -p echo_rust_learning --no-deps

# 检查整个 workspace，而不是只检查当前 package
cargo check --workspace
```

`check` 适合快速迭代，`test` 会编译并执行测试，`run` 会运行目标，`doc` 会执行文档测试相关的
编译。`-p` 选择 package，`--example` 选择 target，`--workspace` 扩大 package 范围。这三个维度
不要混为一谈。

构建结果默认存入共享的 `target/`。第一次编译慢，后续会复用增量缓存；不要把 `target/` 提交到
Git，也不必每次学习前运行 `cargo clean`。

## module 如何映射到文件

[`echo-rust-learning/src/lib.rs`](../../src/lib.rs) 声明顶层模块：

```rust
pub mod basics;
pub mod smart_pointers;
```

编译器会为 `basics` 查找 `src/basics.rs` 或 `src/basics/mod.rs`。在
`smart_pointers/mod.rs` 中继续声明 `pub mod cow;`，形成
`echo_rust_learning::smart_pointers::cow` 路径。

路径关键字：

- `crate::` 从当前 crate 根开始。
- `self::` 从当前 module 开始。
- `super::` 从父 module 开始。
- `echo_agent::` 从外部依赖 crate 开始。

`use` 只是把路径引入当前作用域，不会复制值，也不会改变可见性。`pub use` 则会重导出符号，
为下游提供更稳定、简短的公共路径。

## 可见性不是“能否找到文件”

Rust 默认私有。`pub mod basics` 让外部能进入模块，但模块中的类型和字段仍需各自声明 `pub`：

```rust
pub struct LearningTask {
    pub title: String,
    attempts: u32,
}
```

外部可以读 `title`，不能直接修改 `attempts`，只能通过模块提供的方法维护不变量。还可使用
`pub(crate)` 限制在当前 crate 内，或 `pub(super)` 限制在父模块内。优先暴露行为，而不是把所有
字段都设为公开。

## prelude 是普通模块

`use echo_agent::prelude::*;` 会批量导入框架最常用的公开 API。prelude 不是语言魔法，它只是
包含若干 `pub use` 的普通 module。简短示例可以使用它；生产模块更适合精确导入，以便看清依赖。

## feature 是编译时选择

```rust
#[cfg(feature = "subagent")]
pub mod subagent;
```

feature 未启用时，这段代码根本不进入编译，不是运行到这里才关闭。常见命令：

```bash
cargo check -p echo_agent --features subagent
cargo check -p echo_agent --no-default-features
cargo check -p echo_agent --all-features
```

`--all-features` 不能证明每个 feature 都能独立编译，因为同时启用可能掩盖错误。修改 feature、
可选依赖、`#[cfg]` 或公共 API 后，本项目还要求执行独立 feature 矩阵。

## edition 与版本不是一回事

Rust edition 控制语法和名称解析规则，不等于编译器版本。项目采用 2024 edition，但仍由
`rust-toolchain.toml` 决定具体编译器。升级 edition 通常由 `cargo fix --edition` 辅助；不要只改
manifest 数字就认为迁移完成。

## 在 echo-agent 中定位代码

- [`echo-core/src/lib.rs`](../../../echo-core/src/lib.rs)：核心 trait 和类型入口。
- [`src/lib.rs`](../../../src/lib.rs)：根 crate facade 与 prelude。
- [`echo-rust-learning/src/lib.rs`](../../src/lib.rs)：教学代码入口。
- [根 Cargo.toml](../../../Cargo.toml)：workspace 成员、公共依赖和 feature。

可先用 `rg "pub trait Tool"` 搜定义，再用 rust-analyzer 的“查找引用”确认调用路径。只搜到定义
不代表运行时一定可达；第 15 章会继续讲如何沿真实路径阅读。

## 常见新手错误

1. 在 workspace 根运行 `cargo test`，却误以为覆盖了全部成员；需要明确使用 `--workspace`。
2. 把 package 名和 crate 路径混用，忘记连字符会转为下划线。
3. 写了新文件却忘记在 `mod.rs` 或 `lib.rs` 声明，文件不会参与编译。
4. 只给 module 加 `pub`，却忘了类型或方法仍是私有。
5. 用 `--all-features` 代替独立 feature 检查。

## 练习

1. 用 `cargo metadata --no-deps` 找到 `echo_agent` 的 library、example 和 test targets。
2. 找出 `Tool` 在哪个 crate 定义，又在哪里被根 crate 重导出。
3. 在教学 crate 新建一个私有 module，先观察外部示例的可见性错误，再通过公开函数导出行为。
4. 比较 `cargo check -p echo_agent` 与 `cargo check --workspace` 输出的 package 范围。

下一章：[变量、类型、表达式与控制流](02-language-basics.md)。
