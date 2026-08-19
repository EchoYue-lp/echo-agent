# 07 trait、泛型、宏与 feature

trait 描述行为契约，泛型让同一算法适用于多种具体类型，trait object 则在运行时选择实现。
`echo-agent` 的工具、模型客户端、存储、审批、压缩器都围绕这些机制组织。

运行本章示例：

```bash
cargo run -p echo_rust_learning --example chapter_07_traits_generics
```

代码位于 [`traits.rs`](../../../echo-rust-learning/src/traits.rs)。

## 为什么需要泛型

没有泛型时，相同算法要为每种类型重复实现。泛型把“类型不同、逻辑相同”的部分参数化：

```rust
fn first<T>(items: &[T]) -> Option<&T> {
    items.first()
}
```

这里不需要知道 T 的能力，只返回借用。如果函数要比较、打印或 clone T，必须通过 trait bound 明确
声明。泛型不是动态类型；每个调用点仍有确定类型，并接受完整编译检查。

### 泛型 struct 与 enum

```rust
struct Page<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}
```

`Option<T>`、`Result<T, E>`、`Vec<T>` 都是泛型类型。设计公共类型时，泛型参数过多会把复杂度传给
所有调用者；仅在实际存在多种合理实现时参数化。

## 定义和实现 trait

```rust
pub trait TaskFormatter: Send + Sync {
    fn format(&self, task: &LearningTask) -> String;
}

impl TaskFormatter for PlainFormatter {
    fn format(&self, task: &LearningTask) -> String {
        format!("task: {}", task.title)
    }
}
```

trait 可以包含默认方法、关联类型、关联常量和返回 Future 的方法。公共 trait 的每个新增必需方法
都会影响所有实现方，因此默认实现和最小接口很重要。

### 默认方法

```rust
trait Named {
    fn name(&self) -> &str;

    fn display_name(&self) -> String {
        self.name().to_uppercase()
    }
}
```

默认方法减少重复，但不能掩盖实现之间真正不同的语义。公共 trait 新增有默认实现的方法通常比新增
必需方法兼容性更好。

## 静态分发与泛型约束

```rust
pub fn format_static<F: TaskFormatter>(formatter: &F, task: &LearningTask) -> String {
    formatter.format(task)
}
```

编译器会为具体 F 生成专门代码。约束也可写在 `where` 子句，复杂生命周期和多个 trait 约束时
通常更易读。

```rust
fn render_all<T, F>(items: &[T], formatter: F) -> Vec<String>
where
    F: Fn(&T) -> String,
{
    items.iter().map(formatter).collect()
}
```

trait bound 是函数契约。不要先写宽泛的 `T: Clone + Debug + Send + Sync + 'static` 再说；只声明
实现真正需要的能力，让更多类型可复用函数。

`impl Trait` 常用于参数或返回值：

```rust
fn name(value: impl Into<String>) -> String {
    value.into()
}
```

返回位置的 `impl Trait` 表示“一个确定但不公开的具体类型”，不同分支仍必须返回同一个具体类型。

## 关联类型与泛型参数

关联类型表示“每个实现选择一个确定类型”：

```rust
trait TaskSource {
    type Error;
    fn load(&self) -> Result<Vec<LearningTask>, Self::Error>;
}
```

如果同一个类型需要以不同 Item 多次实现 trait，使用泛型参数；如果每个实现只有一个自然选择，
关联类型通常让调用更简洁。Iterator 的 `type Item` 就是关联类型。

## blanket impl 与组合

对所有满足约束的类型实现 trait 称为 blanket implementation：

```rust
impl<T: TaskFormatter + ?Sized> TaskFormatter for Box<T> {
    fn format(&self, task: &LearningTask) -> String {
        (**self).format(task)
    }
}
```

标准库和框架常用 blanket impl 让 Box/Arc 自动转发行为。实现范围过宽可能与未来实现冲突，公共 API
应谨慎设计。

## 一致性与孤儿规则

通常只有“trait 在当前 crate”或“被实现类型在当前 crate”时才能写 impl。这样不同 crate 不会为
同一对外部类型提供冲突实现。需要为两个外部类型增加行为时，常用 newtype 包装其中一个类型。

## trait object 与对象安全

`dyn TaskFormatter` 擦除具体类型，通过虚函数表调用。并非所有 trait 都能直接变成 trait object；
方法如果返回 `Self`、使用不受约束的泛型参数，通常需要重新设计或加 `where Self: Sized`。

框架的 `Tool`、`Agent`、`LlmClient` 设计成可用于动态分发，因此可以按配置组合实现。

### `dyn Trait` 的内存表示

`&dyn Trait`、`Box<dyn Trait>` 通常是胖指针：一个指向数据，一个指向 vtable。vtable 保存方法地址、
大小、对齐和析构信息。动态分发的代价通常很小，但会阻止部分内联；选择它的主要依据是是否需要
异构集合、运行时插件或稳定边界，而不是先做微优化。

### 对象安全

要形成 trait object，方法必须能通过 vtable 调用。典型障碍：

```rust,compile_fail
trait Factory {
    fn create<T>(&self) -> T;
    fn clone_self(&self) -> Self;
}
```

泛型方法没有单一 vtable 入口，返回裸 Self 也不知道大小。可以把只适合具体类型的方法加
`where Self: Sized`，或把泛型移动到关联类型/另一个 trait。

### `Send + Sync` 是并发契约

```rust
pub trait Tool: Send + Sync { /* ... */ }
```

这要求所有 Tool 实现都能安全在线程间移动和共享。它不是“性能标记”，而是编译器检查的安全
属性。包含 Rc/RefCell 的实现通常无法满足，需要重新考虑所有权，而不是强行绕过。

## Builder 模式

字段较多、默认值多、构建可能失败时，Builder 比长参数构造函数清晰：

```rust
let task = LearningTaskBuilder::new()
    .title("理解 trait")
    .running(true)
    .build()?;
```

Builder 方法通常接收 `mut self` 并返回 `Self`，链式调用会逐步转移 Builder 所有权。最终 `build`
执行跨字段校验并返回 Result。

Builder 的两种常见接收者：

- `fn field(mut self, ...) -> Self`：链式构建，逐步移动 Builder。
- `fn field(&mut self, ...) -> &mut Self`：原地配置，适合复用一个可变 Builder。

两种都合理，但同一 API 应保持一致。最终 build 是否消耗 Builder 取决于能否重复构建和字段所有权。

## derive 宏与属性宏

derive 宏根据类型定义生成实现：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config { /* fields */ }
```

属性宏可以转换整个函数或 impl。`#[tool]` 会根据异步函数生成参数结构、JSON Schema 和 `Tool`
实现。教学 crate 的
[`project_patterns.rs`](../../../echo-rust-learning/src/project_patterns.rs) 定义真实工具，并直接调用
生成的工具类型，不经过 LLM：

```bash
cargo run -p echo_rust_learning --example chapter_15_echo_agent_tool
```

宏展开后仍受普通 Rust 的所有权、可见性和 trait 规则约束。遇到难懂的宏错误时，先确认输入函数
签名是否满足宏契约，再查看 `echo-macros` 的生成逻辑。

Rust 宏分为：

- 声明宏 `macro_rules!`：按 token 模式展开，例如项目的消息构造宏。
- derive 过程宏：从 struct/enum 生成 trait 实现。
- 属性过程宏：转换函数、impl 或 module。
- 函数式过程宏：以 `name!(...)` 形式接收 token 并生成代码。

宏发生在编译期，不能替代运行时校验。生成的公开类型和错误也是 API 的一部分，需测试宏在 facade
依赖和底层 crate 依赖两种路径下能否解析。

## feature gate

```rust
#[cfg(feature = "mcp")]
pub mod mcp;
```

feature 用于可选能力和依赖隔离。不要把 EKO 产品策略包装成通用 feature 塞入框架；也不要只在
all-features 下编译，因为独立 feature 可能缺失它真正需要的依赖。

`cfg!` 返回运行时 bool，但两个分支仍参与类型检查；`#[cfg]` 则直接移除未启用代码。可选依赖通常由
feature 通过 `dep:name` 或子 crate feature 激活。feature 是可加的：依赖图中任一方启用后都会
生效，因此不能用 feature 表达互斥的运行时产品模式。

## 项目映射

- [`Tool`](../../../echo-core/src/tools/mod.rs)：对象安全的异步工具接口。
- [`LlmClient`](../../../echo-core/src/llm/mod.rs)：模型协议抽象。
- [`ReactAgentBuilder`](../../../src/agent/react/builder.rs)：大型 Builder 和 `Arc<dyn Trait>` 注入。
- [`echo-macros/src/lib.rs`](../../../echo-macros/src/lib.rs)：过程宏入口。
- [`Cargo.toml`](../../../Cargo.toml)：根 feature 拓扑。

## 练习

1. 增加一个 Markdown formatter，并同时通过泛型函数和 trait object 调用。
2. 为教学工具增加第二个参数，观察 `#[tool]` 生成的参数校验行为。
3. 找一个只在 feature 下存在的公开类型，验证 no-default-features 是否仍能编译 facade。
4. 定义带关联类型的 TaskSource，并实现内存版本。
5. 写一个因为泛型方法而不对象安全的 trait，再用 `Self: Sized` 拆分具体类型方法。

下一章：[Box、Deref、Drop 与 RAII](08-smart-pointers-foundations.md)。
