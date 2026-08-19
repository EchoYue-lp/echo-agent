# 13 Serde、JSON、配置与协议边界

Agent 框架不断在 Rust 类型、模型 JSON、配置文件、事件流和持久化文件之间转换。Serde 把“Rust
值如何序列化/反序列化”抽象为 trait，具体格式由 `serde_json`、`serde_yaml_ng` 等 crate 实现。

```bash
cargo run -p echo_rust_learning --example chapter_13_serde
```

## 序列化与反序列化方向

- `Serialize`：从可信 Rust 值生成 JSON、YAML 等表示。
- `Deserialize<'de>`：从外部输入尝试构造 Rust 值。

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub name: String,
    pub max_iterations: usize,
    #[serde(default)]
    pub tools: Vec<String>,
}
```

derive 宏生成 trait 实现，但不会自动验证 `name` 非空、迭代次数有上限等领域规则。反序列化只说明
输入符合结构和基本类型。

教学实现见 [`serialization.rs`](../../../echo-rust-learning/src/serialization.rs)，示例执行 JSON
round-trip：Rust 值编码为 JSON，再解码回来并比较关键字段。

## JSON 值如何映射到 Rust

| JSON | 常见 Rust 类型 | 注意点 |
|------|----------------|--------|
| string | `String`、`&str` | 借用反序列化受输入生命周期限制 |
| number | `i64`、`u64`、`f64` | 必须明确范围和精度 |
| boolean | `bool` | 不把字符串 `"true"` 当布尔值 |
| null | `Option<T>` | 缺字段与 null 的处理可因属性不同 |
| array | `Vec<T>` | 仍需限制长度和元素规则 |
| object | struct、map | struct 更能表达固定 schema |

配置通常拥有解析结果，因此使用 `String` 最直接。高性能只读解析可以借用输入，但会把输入 buffer
的生命周期传遍 API；没有测量依据时不要过早增加这种复杂度。

## 常用字段属性

```rust
#[serde(rename_all = "snake_case")]
enum State {
    InProgress,
    Completed,
}

#[derive(Serialize, Deserialize)]
struct Response {
    #[serde(default)]
    tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}
```

- `rename` / `rename_all` 固定 wire name，不让 Rust 标识符风格泄漏到协议。
- `default` 在字段缺失时使用 `Default` 或指定函数。
- `skip_serializing_if` 只影响输出，不自动定义输入缺失规则。
- `alias` 可接受额外输入名称；是否保留旧名称属于明确协议决策。
- `flatten` 合并嵌套对象字段，灵活但可能隐藏冲突并降低 schema 清晰度。
- `deny_unknown_fields` 能拒绝拼错字段，但会让扩展字段不再向前兼容，应按边界选择。

本项目尚在开发期，不需要为过时代码和旧 schema 保留兼容层；但当前同一版本内的事件生产者和
消费者仍必须共享精确 wire contract。

## enum 的三种常见表示

外部标签（默认）：

```json
{ "completed": { "summary": "done" } }
```

内部标签适合所有变体都是对象：

```rust
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Started { task_id: String },
    Completed { summary: String },
}
```

相邻标签把类型和内容分开：

```rust
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum Event {
    Token(String),
    Completed { summary: String },
}
```

对应输出为 `{"type":"token","data":"hello"}`。标签方式是协议的一部分，改变它必须同步
生产者、消费者、文档和契约测试。

未标签 enum 会尝试逐个匹配变体，错误可能模糊且新增变体会改变匹配结果；对公开事件协议通常
优先显式标签。

## Option、缺字段与 null

`Option<T>` 表示值可以合法缺失。对默认 derive，字段缺失和 `null` 通常都会得到 `None`，但这不
一定符合业务语义。例如“调用方没提供”与“调用方明确清空”在 PATCH 请求中可能完全不同，此时
需要额外 enum 或双层 Option 精确建模，而不是把差异丢掉。

`#[serde(default)] Vec<T>` 会把缺字段变为空集合。只有“缺失确实等价于空”时才这样做，否则应让
反序列化返回缺字段错误。

## Value 只停留在适配边界

`serde_json::Value` 适合接收模型生成的未知工具参数或转发第三方扩展字段。进入已知领域逻辑后，
应尽快用 `serde_json::from_value` 转为强类型：

- struct 在编译期固定字段类型。
- enum 明确有限状态，不接受任意字符串。
- Option 明确合法缺失。
- 结构化反序列化错误带有字段上下文。

层层调用 `value.get("field")` 会让错误延迟、分支分散，还容易把字段拼写错误当作“没有值”。
`#[tool]` 宏的职责之一就是在边界把无类型 JSON 参数转换为 Rust 参数，再调用业务函数。

## 解析与验证必须分层

建议处理路径：

```text
原始 bytes/text
    -> 格式解析（JSON 是否合法）
    -> 类型反序列化（字段类型是否匹配）
    -> 领域验证（范围、组合、不变量）
    -> 已验证领域对象
```

例如 `max_iterations: usize` 仍可能是 0 或异常大；`name: String` 仍可能只含空白。可以提供
`validate()`，或让构造器生成已验证 newtype。不要在后续执行深处才发现配置无效。

错误也应保留层次：语法错误、字段类型错误、缺字段和领域不变量失败对用户的修复方式不同。错误链
可通过 `source` 保留底层原因，同时在上层补充“读取哪个文件/解析哪个事件”的上下文。

## 自定义序列化何时需要

derive 足以覆盖大部分情况。只有 wire 格式无法通过属性表达、需要额外验证或与第三方固定协议适配
时才手写 `Serialize`/`Deserialize`。实现 visitor 容易遗漏类型和错误位置，应配套正反例测试，且
绝不在外部输入上使用可能 panic 的操作。

## 配置、状态与事件应是不同类型

- 配置表达用户希望系统如何运行。
- 运行时状态表达当前执行进展。
- 事件表达某个时间点发生的变化。
- 持久化快照表达可恢复的数据集合。

即使字段相似，也不要复用一个万能 struct。不同类型让可变性、默认值、协议稳定性和校验责任更
清楚，也避免 UI 临时字段污染框架领域模型。

## 敏感信息与日志

API Key 等秘密可能必须反序列化，但不应由 `Debug`、错误或 trace 原样打印。敏感类型可自定义
`Debug` 为脱敏输出，序列化配置时也要明确哪些字段允许落盘。这是本地应用同样成立的安全边界。

## 项目映射

- [`echo-core/src/agent/mod.rs`](../../../echo-core/src/agent/mod.rs)：带标签的 AgentEvent。
- [`echo-core/src/llm/types.rs`](../../../echo-core/src/llm/types.rs)：模型消息 wire types。
- [`src/agent/config.rs`](../../../src/agent/config.rs)：Agent 配置。
- [`file_conversation.rs`](../../../echo-state/src/memory/file_conversation.rs)：文件持久化。

## 练习

1. 给 `AgentProfile` 增加可选 description，输出时忽略 `None`，补 round-trip 测试。
2. 分别输入“缺少 tools”“tools 为 null”“tools 类型错误”，记录三种结果。
3. 为配置增加 `validate()`，拒绝空白名称和为零的迭代次数。
4. 为一个相邻标签 enum 写精确 JSON 契约测试，验证 wire name。

下一章：[测试、文档与静态检查](14-testing-and-tooling.md)。
