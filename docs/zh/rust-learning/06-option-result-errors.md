# 06 Option、Result 与错误处理

Rust 用类型区分“值可能不存在”和“操作可能失败”，而不是依赖 null、异常或约定俗成的返回码。

先运行：

```bash
cargo run -p echo_rust_learning --example chapter_06_errors
```

示例展示一条成功路径和一条预期失败路径。错误被当成普通值匹配，进程不会因为无效输入退出。

## `Option<T>` 表示缺失

```rust
let model: Option<String> = config.default_model;
```

常用组合方法：

- `map`：只转换 Some 内部值。
- `and_then`：下一步也返回 Option 时继续链式处理。
- `or_else`：按需计算回退值。
- `as_ref`/`as_deref`：把拥有的 Option 转成借用视图。
- `is_some_and`：在存在时检查条件。
- `zip`：两个值都存在时配对。
- `take`：从 `&mut Option<T>` 中取走值并留下 None。

需要提前返回时，`let Some(value) = option else { return ... };` 往往比多层嵌套更清晰。

### 什么时候应该是 Option

- 查找不到是正常结果，例如 `registry.get(name)`。
- 某配置字段确实可省略。
- 事件 payload 只在部分变体有值。

如果调用者需要知道“为什么没有”，Option 信息不足，应改用 Result。不要用 None 同时表示未找到、
权限拒绝、解析失败和已取消。

### 借用 Option 内部值

对 `Option<String>` 直接 match 可能移走 String。只读时用：

```rust
let model: Option<&str> = config.model.as_deref();
```

`as_ref` 得到 `Option<&String>`，`as_deref` 进一步利用 Deref 得到 `Option<&str>`。

## `Result<T, E>` 表示成功或失败

```rust
pub fn parse_positive_limit(input: &str) -> Result<usize, LearningError> {
    let value = input
        .parse::<usize>()
        .map_err(|_| LearningError::InvalidLimit(input.to_string()))?;
    if value == 0 {
        return Err(LearningError::InvalidLimit(input.to_string()));
    }
    Ok(value)
}
```

`?` 在成功时取出值，失败时把错误转换成当前函数的错误类型并提前返回。它不会吞掉错误，也不会
自动记录日志。

`?` 只能用于返回兼容 Result/Option 的上下文。main、测试和 async 函数都可以直接返回 Result，
使准备步骤保持线性：

```rust
fn load_settings(name: Option<&str>, limit: &str) -> Result<(String, usize), LearningError> {
    let name = required_text("name", name)?;
    let limit = parse_positive_limit(limit)?;
    Ok((name, limit))
}
```

完整代码见 [`errors.rs`](../../../echo-rust-learning/src/errors.rs)。

## Result 组合子

- `map`：转换 Ok。
- `map_err`：转换 Err 或增加领域语义。
- `and_then`：成功后继续执行另一个可能失败的步骤。
- `or_else`：只对错误执行恢复逻辑。
- `inspect`/`inspect_err`：观察而不改变值，适合轻量诊断。

组合子适合短转换；业务分支多时 match 更清晰。不要为了“函数式”把五种错误恢复压成一条难读链。

## `From`、`Into` 与 `?` 的错误转换

`?` 遇到错误时概念上调用 `From::from(error)`。`thiserror` 可以生成转换：

```rust
#[derive(Debug, thiserror::Error)]
enum ConfigError {
    #[error("cannot read config: {0}")]
    Io(#[from] std::io::Error),
}
```

如果两个底层错误需要不同领域语义，不要都粗暴变成 String；保留结构化变体和 source，调用者才能
分类重试、展示和审计。

## 错误分层

好的错误类型表达调用者能采取的动作：

- 参数错误：调用者可修正输入。
- 网络/限流错误：可能重试。
- 权限拒绝：需要用户确认或改变策略。
- 取消：不是普通失败，不应伪装成错误文本。
- 内部不变量破坏：返回结构化框架错误并附带上下文。

[`LearningError`](../../../echo-rust-learning/src/errors.rs) 使用 `thiserror` 生成 `Display` 和
`Error` 实现。框架的 [`ReactError`](../../../echo-core/src/error.rs) 进一步用 enum 汇总各子系统
错误，并通过 `From` 支持 `?`。

`Display` 面向用户或日志，`Debug` 面向开发诊断，`Error::source` 保留原因链。对外错误消息不应
包含密钥、完整请求头或未经控制的大段模型内容。

## Option 与 Result 之间转换

缺失本身是错误时：

```rust
let title = title.ok_or(LearningError::EmptyTitle)?;
```

只想忽略某类失败时必须明确说明原因，不能对所有错误一概 `.ok()`。错误一旦被转成 None，调用者
会失去失败原因。

反过来，一个 Option 可以通过 `transpose` 和 Result 组合。例如 `Option<Result<T, E>>` 转成
`Result<Option<T>, E>`，适合“字段可缺失，但存在时必须解析成功”。

## 可恢复错误、Bug 与不变量

外部输入不合法、文件不存在、网络超时、用户取消都应进入 Result。程序员写错索引或违反内部
不变量属于 bug，但本项目仍要求框架不要用 panic 扩大破坏范围，应在边界返回结构化错误。

测试中的 `assert!` 失败是测试报告机制，不等于生产实现可以 panic。生产库不能依赖“调用者一定
传对”来使用危险提取 API。

## 为什么项目禁止 panic 路径

用户文本、模型输出、工具参数、网络数据和配置都可能异常。一个工具的坏输入不应让整个本地 Agent
进程退出。因此生产路径不使用可能 panic 的提取和索引方式，而是：

- 用 `get()` 获取集合元素。
- 用 `ok_or` 把 None 转成错误。
- 用 `map_err` 添加领域语义。
- 用 `unwrap_or` 提供明确、安全的默认值。
- 用 `checked_*`/`saturating_*` 处理整数边界。
- 用 `chars()` 处理 UTF-8 用户文本。

测试断言可以失败以报告回归，但生产实现必须返回 `Result` 或处理缺失分支。

## 取消、超时与失败必须区分

教学 `LearningError` 分别定义 Cancelled、TimedOut 和 ChannelClosed。三者后续动作不同：

| 终止原因 | 常见处理 |
|----------|----------|
| Cancelled | 停止派生任务，不自动当成系统故障重试 |
| TimedOut | 根据幂等性和策略决定重试或提示 |
| ChannelClosed | 判断消费者正常退出还是内部通路异常 |
| Invalid input | 请求调用者修正，不原样重试 |

把它们都变成 `Err("failed")` 会破坏运行时状态、UI 呈现和重试策略。

## 添加上下文，但不破坏分类

底层 IO 错误只说“not found”时，上层应补充正在读哪个配置/工件。可以在领域变体中保存 path 和
source，而不是只 `map_err(|e| e.to_string())`。字符串化过早会失去可检查的错误类型。

## 不要重复记录同一个错误

通常由最了解上下文、且确定错误将被终止或降级处理的边界记录日志。底层函数既 log 又返回错误，
上层再 log，会产生重复噪声。底层优先补充结构化上下文并返回。

## 运行示例

```bash
cargo run -p echo_rust_learning --example chapter_06_errors
cargo test -p echo_rust_learning errors
```

## 项目映射

- [`echo-core/src/error.rs`](../../../echo-core/src/error.rs)：统一错误域和终止类别。
- [`echo-state/src/memory/store.rs`](../../../echo-state/src/memory/store.rs)：IO/序列化/锁错误转换。
- [`echo-execution/src/tools.rs`](../../../echo-execution/src/tools.rs)：工具失败分类和恢复动作。

## 练习

1. 给 `LearningError` 增加一个包含字段名的配置错误，并用 `map_err` 转换 JSON 错误。
2. 找一个嵌套 Option 链，分别用 match 和 `and_then` 重写，比较可读性。
3. 设计“取消”和“超时”的不同 enum 变体，解释调用者应如何区别处理。
4. 将 `Option<Result<u32, _>>` 用 transpose 转成 Result<Option<u32>, _>。
5. 定义一个包含 path 和 `#[source]` IO 错误的变体，打印 source chain。

下一章：[trait、泛型、宏与 feature](07-traits-generics-macros.md)。
