# 14 测试、rustdoc、fmt、Clippy 与编译器诊断

Rust 编译器能证明类型、借用和线程安全约束，却不能证明业务行为正确。高质量修改需要让编译器、
单元测试、集成测试、文档测试和静态检查各自负责一层问题。本章先教如何写测试，再教如何阅读失败。

## 一个测试只说明一个行为

推荐使用 Arrange、Act、Assert 结构：

```rust
#[test]
fn preview_appends_suffix_after_character_limit() {
    let input = "中文🦀Rust";

    let preview = unicode_preview(input, 3);

    assert_eq!(preview, "中文🦀...");
}
```

测试名描述输入条件和预期行为，不写泛化的 `test_preview`。断言失败时，新手无需重新推断这个测试
本来想验证什么。边界测试通常比重复 happy path 更有价值：空输入、零、最大值、Unicode、多次
调用、关闭 channel 和错误分支都应按风险覆盖。

## 单元测试

单元测试放在被测 module 的 `#[cfg(test)] mod tests` 中，可以访问私有实现：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_valid_task() -> Result<(), LearningError> {
        let task = LearningTask::new("test")?;
        assert_eq!(task.title, "test");
        Ok(())
    }
}
```

测试返回 `Result` 后，可以对准备步骤使用 `?`，而不是强制提取。若需要断言某个具体错误，可用
`matches!` 或比较稳定的错误变体，不要只比较可能变化的完整展示文本。

私有纯函数、状态转换和边界条件适合单元测试。测试不应完全复制实现，否则重构时两份算法一起改，
却没有验证真实契约。

## 集成测试

`tests/` 下每个 `.rs` 文件都是独立 crate，只能通过公开 API 使用被测库。这能发现“内部单测都过，
但公共导出、feature 或类型签名不可用”的问题。

教学 crate 的 [`learning_contract.rs`](../../tests/learning_contract.rs) 验证：

- 中文与 emoji 预览不会破坏 UTF-8。
- Weak 注册表不会延长对象生命周期。
- 并发与真实 `#[tool]` 示例可以完全离线运行。

集成测试适合跨 module 的用户路径，不必重复每个私有分支。对于数据库、文件或网络边界，优先使用
临时目录和可控 fake，避免依赖开发者机器的现有状态。

## doctest：让 API 示例保持可编译

库源码中的 `///` 和 `//!` 会生成 rustdoc。Rust fenced code 默认作为 doctest 编译或运行：

```rust
/// 返回不超过 `limit` 个 Unicode 标量值的预览。
///
/// ```
/// use echo_rust_learning::basics::unicode_preview;
/// assert_eq!(unicode_preview("中文Rust", 2), "中文...");
/// ```
```

稳定、短小、离线的公共 API 示例应参与 doctest。只需编译而不应执行可标 `no_run`；故意展示借用
错误可标 `compile_fail`；纯伪代码才标 `ignore`。不要用 `ignore` 隐藏本可修复的过时示例。

```bash
cargo test --doc -p echo_rust_learning
cargo doc -p echo_rust_learning --no-deps --open
```

## compile-fail 测试为何重要

所有权 API 的一部分契约是“某些误用必须编译失败”，例如把 `Rc` 发送到线程、同时保持冲突借用。
`compile_fail` doctest 或专门 UI 测试可固定这些约束。错误文本会随编译器变化，因此优先验证是否
失败和关键诊断，不要脆弱地匹配整段输出。

## 异步测试要保持确定性

```rust
#[tokio::test]
async fn cancellation_is_distinct_from_timeout() {
    // 安排输入，等待结果，断言结构化错误变体。
}
```

不要断言两个并发任务的打印先后，除非顺序就是协议。对于时间行为：

- 让完成时长与期限明显分离，减少 CI 抖动。
- 能使用暂停时钟和手动推进时，不等待真实秒数。
- 用 channel 或 barrier 协调测试阶段，不用“睡一会儿猜它已启动”。
- 测试结束时收集或取消所有 spawn 任务，避免泄漏到后续测试。

## fake、mock 与真实边界

trait 让测试可以注入可控实现。fake 是有简单真实行为的替代实现，例如内存 Store；mock 主要验证
交互次数和参数。优先断言对用户可见的输出和状态，只有调用顺序本身是协议时才使用严格 mock，
否则重构内部实现会造成大量无意义测试修改。

网络模型测试不应默认调用真实服务。把 LLM、工具和 Store 放在 trait 边界，用固定响应 fake 验证
Agent 流程；真实服务另设显式集成测试。

## 属性测试与表驱动测试

一个 UTF-8 截断函数可用多组输入做表驱动测试，也可用属性测试表达不变量：输出永远是有效 UTF-8、
保留字符数不超过限制、未超限时不加后缀。属性测试适合输入空间大、规则稳定的纯函数；状态复杂或
错误难定位时，先保留几个具名边界案例。

## Cargo test 的过滤与输出

```bash
# 当前 package 全部 target 测试
cargo test -p echo_rust_learning --all-targets

# 名字包含 ownership 的测试
cargo test -p echo_rust_learning ownership

# 只运行集成测试文件
cargo test -p echo_rust_learning --test learning_contract

# 需要查看测试打印时
cargo test -p echo_rust_learning test_name -- --nocapture
```

过滤后的快检只服务开发循环，不能代替提交门禁。看到“0 tests”时要确认过滤条件是否写错，不能把
命令成功退出误认为目标测试已执行。

## fmt 与 Clippy

```bash
cargo fmt --all
cargo fmt --all -- --check

cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy --workspace --lib --bins --all-features --locked -- \
  -D clippy::unwrap_used \
  -D clippy::expect_used \
  -D clippy::panic \
  -D clippy::unreachable
```

rustfmt 统一排版；第一条会写入，第二条验证没有剩余 diff。Clippy 检查可疑和不惯用代码，本项目
把 warning 和会导致 panic 的高风险 API 作为错误。不要用大范围 `allow` 让命令变绿；确有误报时
只在最小位置解释安全不变量。

## 如何阅读编译错误

按以下顺序处理：

1. 从第一条根因错误开始，后续可能都是连锁错误。
2. 看 `expected` / `found`、变量定义处和 borrow 最后使用处。
3. 判断所有权意图：该 move、borrow、短暂 clone，还是确实需要 Arc？
4. 生命周期错误先画出引用来源和使用范围，不要随手改为 `'static`。
5. async 错误检查哪些值跨 `.await`，spawn 是否要求 `Send + 'static`。
6. 宏错误可先缩小输入签名，再用相关宏文档理解生成代码约束。

编译器建议是线索，不是领域设计决定。为了消除错误到处 clone、Box 或 Arc，往往会让真实所有权
更难理解。

## 测试失败如何定位

1. 单独运行最小失败测试并开启需要的输出。
2. 区分编译失败、测试准备失败和断言失败。
3. 检查测试是否依赖顺序、时间、环境变量或全局状态。
4. 修复根因后重新跑相关 crate，而不只跑刚才的单个测试。
5. 提交前按仓库 `AGENTS.md` 执行完整适用门禁。

任何失败都应修复，包括看似“与本次改动无关”的失败；否则测试套件会逐渐失去可信度。

## 文档也是契约

[`tests/documentation_contract.rs`](../../../tests/documentation_contract.rs) 会递归检查本仓库 Markdown
本地链接。示例名、文件名和章节号变化时，索引与上下章导航也必须一起更新。

## 练习

1. 为 `ensure_period` 增加 Borrowed、Owned、空字符串三组测试。
2. 给一个所有权误用写 `compile_fail` doctest，并解释关键诊断。
3. 为异步取消路径写不依赖输出顺序的测试。
4. 故意给示例写错 package 名，按本章顺序阅读并修复 Cargo 错误。

下一章：[阅读和修改 echo-agent 源码](15-reading-echo-agent.md)。
