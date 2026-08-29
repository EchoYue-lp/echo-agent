# 03 struct、enum、方法与模式匹配

基础类型表达“数据是什么”，领域类型表达“系统允许哪些状态和操作”。`echo-agent` 中的工具结果、
运行事件、任务状态和消息内容都依赖 struct 与 enum 建模。

## 运行本章示例

```bash
cargo run -p echo_agent_learning --example chapter_03_domain_modeling
```

对应实现是 [`basics.rs`](../../src/basics.rs)。

## struct：具名字段组成一个值

```rust
pub struct LearningTask {
    pub title: String,
    pub state: TaskState,
}
```

struct 的大小在编译期确定。字段默认私有；示例为了教学开放字段，生产领域对象往往把不变量相关
字段设为私有，只通过方法修改。

### 构造语法与字段简写

```rust
let title = String::from("read");
let task = LearningTask {
    title, // 等价于 title: title
    state: TaskState::Pending,
};
```

从另一个值更新字段可以用 `..other`，但要注意没有实现 Copy 的字段可能被 move。

### tuple struct 与 newtype

tuple struct 的字段没有名字：

```rust
struct RunId(String);
struct ConversationId(String);
```

虽然底层都是 String，两个 newtype 不能互换，能防止把会话 ID 误传到 run ID 参数。只有字段含义
非常明确时才用普通 tuple struct；否则具名字段更易读。

### unit struct

没有字段的类型如 `struct PlainFormatter;` 不占业务数据，可用于无状态策略或 trait 实现。它仍然是
独立类型，不等于 `()`。

## `impl`：方法与关联函数

```rust
impl LearningTask {
    pub fn new(title: impl Into<String>) -> Result<Self, LearningError> { /* ... */ }
    pub fn start(&mut self) -> bool { /* ... */ }
    pub fn summary(&self) -> Option<&str> { /* ... */ }
}
```

- `Self` 表示当前类型。
- 没有 `self` 参数的是关联函数，通过 `LearningTask::new()` 调用。
- `&self` 只读借用。
- `&mut self` 可变借用。
- `self` 消耗当前值，常用于 Builder 的链式方法或状态转换。

方法接收者体现所有权意图。不要为了通过编译器而随意把 `&self` 改成 `&mut self` 或 `self`。

## enum：一组互斥变体

```rust
pub enum TaskState {
    Pending,
    Running,
    Completed { summary: String },
}
```

同一个 TaskState 在同一时刻只能是其中一个变体。`Completed` 自带 summary，因此不会出现
“状态 completed，但 summary 是 None”这种非法组合。

Rust enum 的大小约等于“最大 payload + 变体标签 + 对齐填充”。如果一个变体特别大，整个 enum
都变大；第 8 章会解释为什么框架有时用 Box 缩小变体。

## Option 和 Result 也是 enum

```rust
enum Option<T> { None, Some(T) }
enum Result<T, E> { Ok(T), Err(E) }
```

它们没有 null 或异常魔法，所有 `map`、`and_then`、`?` 都建立在 enum 与 trait 上。第 6 章会
深入错误传播。

## 模式匹配会解构值

```rust
match &task.state {
    TaskState::Completed { summary } => Some(summary.as_str()),
    TaskState::Pending | TaskState::Running => None,
}
```

这里匹配的是 `&TaskState`，所以 `summary` 是借用，不会把 String 从 task 中 move 出去。新手常见
错误是对拥有的字段匹配后又继续使用原对象，编译器会报告 partial move。

### `if let`

只关心一个模式时：

```rust
if let TaskState::Completed { summary } = &task.state {
    println!("{summary}");
}
```

### `let else`

失败分支需要立即退出时：

```rust
let Some(summary) = task.summary() else {
    return Ok(());
};
```

### 匹配守卫

```rust
match attempts {
    value if value > limit => "over limit",
    0 => "not started",
    _ => "running",
}
```

守卫适合额外布尔条件，不应替代能由更准确 enum 变体表达的领域状态。

## `derive` 自动实现常用 trait

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
```

- `Debug`：开发日志和断言输出。
- `Clone`：显式深/共享复制，取决于字段的 Clone 行为。
- `PartialEq`/`Eq`：比较和测试。
- `Default`：合理的默认值。
- Serde derive：第 13 章详解。

derive 只有在所有字段都满足相应 trait 时才成功。不要为“方便”给不应该复制的资源句柄派生 Clone。

## 用类型维护不变量

设计领域类型时依次提问：

1. 哪些状态互斥？用 enum。
2. 哪些数据只在某状态存在？放进对应变体。
3. 哪些字段组合必须一起校验？用构造函数返回 Result。
4. 哪些标识符底层相同但语义不同？考虑 newtype。
5. 哪些状态转换不允许？通过方法限制，而不是公开任意字段修改。

## UTF-8 是领域边界的一部分

[`unicode_preview`](../../src/basics.rs) 使用 `chars().take()`。`String` 不能用整数
直接索引，因为 UTF-8 字符宽度不固定。`str::len()` 是字节数；用户可见文本限制至少应按 chars
计算，本项目严禁不安全的字节截断。

## 常见错误

| 错误 | 原因 | 修复思路 |
|------|------|----------|
| use of partially moved value | match/字段访问移走了内部 String | 对 enum 或字段加 `&` 借用 |
| cannot borrow as mutable | 方法需要 `&mut self`，调用者没有可变绑定 | 判断状态是否真的应变化，再加 `mut` |
| non-exhaustive patterns | enum 新增变体未处理 | 明确新变体语义，不急着用 `_` 吞掉 |
| trait bound `Clone` not satisfied | derive 依赖字段也实现 Clone | 判断资源是否应共享/重建，而非机械 Clone |

## 项目映射

- [`AgentEvent`](../../../echo-core/src/agent/mod.rs)：带 payload 的生命周期事件 enum。
- [`ToolResult`](../../../echo-core/src/tools/mod.rs)：结构化工具终止结果。
- [`ReactError`](../../../echo-core/src/error.rs)：错误域 enum。
- [`TaskState`](../../../echo-orchestration/src/tasks)：任务状态和转换。

## 练习

1. 给教学 TaskState 增加 `Skipped { reason: String }`，修复所有穷尽 match。
2. 把 `LearningTask.title` 改成私有字段，提供只读 getter。
3. 定义 `TaskId(String)` 与 `RunId(String)`，验证它们不能互传。
4. 写一个只接受 Completed 状态并返回 summary 的函数，分别用 match 和 let-else 实现。

下一章：[所有权、借用与生命周期](04-ownership-borrowing-lifetimes.md)。
