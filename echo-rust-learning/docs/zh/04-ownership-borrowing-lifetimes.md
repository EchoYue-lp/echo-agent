# 04 所有权、借用与生命周期

所有权是 Rust 最重要的规则：每个值有一个所有者，所有者离开作用域时值被释放。编译器据此在
没有垃圾回收器的情况下防止悬垂引用、重复释放和多数数据竞争。

先运行配套示例：

```bash
cargo run -p echo_rust_learning --example chapter_04_ownership
```

## 先建立内存模型：栈、堆与作用域

栈按调用顺序保存大小已知的局部值，分配和释放非常快。堆保存大小或生命周期需要动态管理的数据，
程序通过指针找到它。`String` 可以简化理解为栈上的三元组：指针、长度、容量；文本字节位于堆上。

```text
栈上的 String                 堆上的 UTF-8 缓冲区
┌──────────┐                  ┌─────────────────┐
│ pointer ─┼─────────────────>│ 52 75 73 74 ... │
│ length   │                  └─────────────────┘
│ capacity │
└──────────┘
```

所有者离开作用域时，Rust 自动调用 Drop，释放 String 的堆缓冲区。所有权规则确保只释放一次。
“栈值”和“堆值”不是两种互斥的 Rust 类型；很多拥有堆数据的类型同时在栈上保存管理信息。

## move：转移所有权

```rust
let first = String::from("task");
let second = first;
```

`String` 包含堆缓冲区，赋值默认转移所有权。之后只能使用 `second`。像整数、布尔值这类实现
`Copy` 的小型值会按位复制，原绑定仍可使用。

```rust
let first = 8_u32;
let second = first;
println!("{first} {second}"); // u32: Copy
```

实现 Drop 的类型不能同时实现 Copy，因为隐式复制会让资源所有权不明确。`Clone` 是显式操作，
可能深拷贝数据，也可能像 Arc 一样只增加引用计数；不能只看方法名判断成本。

函数参数同样会发生所有权转移。教学函数
[`append_label`](../../src/ownership.rs) 接收整个 `Vec<String>`，修改后再返回，
适合调用者不再需要原集合的场景。

## borrow：临时访问而不取得所有权

```rust
pub fn character_count(value: &str) -> usize {
    value.chars().count()
}
```

调用者传入 `&title`，函数只借用文本。基本规则是：

- 同一时刻可以有多个不可变引用；或一个可变引用。
- 引用不能比它指向的值活得更久。
- 借用结束后，所有者继续拥有并可使用原值。

## 可变借用与别名规则

```rust
fn add_suffix(value: &mut String) {
    value.push_str(" completed");
}
```

持有 `&mut T` 时，当前作用域不能同时通过其他引用访问同一个 T。可以把规则记为：同一时刻要么
多个读者，要么一个写者。它防止迭代时修改集合、引用失效和无锁数据竞争。

下面不能编译：

```rust,compile_fail
let mut value = String::from("task");
let reading = &value;
let writing = &mut value;
println!("{reading} {writing}");
```

不要立即用 clone 绕开。先问：只读引用是否可以更早结束？修改能否移到读取之后？数据是否真的需要
共享所有权？

## 非词法生命周期 NLL

借用通常在最后一次使用处结束，而不是机械延续到花括号末尾：

```rust
let mut title = String::from("draft");
let view = &title;
println!("{view}");       // view 最后一次使用
title.push_str(" done"); // 可以可变借用
```

编译器根据控制流计算借用范围。显式增加内部代码块有时能让作用域更清楚，但不应作为无脑修复。

## `&str`、`String` 与 clone

API 选择可以按意图判断：

| 需求 | 常见类型 |
|------|----------|
| 只读取文本 | `&str` |
| 需要保存、移动或修改文本 | `String` |
| 接受字符串或字符串切片并取得所有权 | `impl Into<String>` |
| 可选的借用文本 | `Option<&str>` |

`clone()` 会显式复制拥有所有权的数据。它不是错误，但应说明为什么需要第二份独立数据。
如果函数只读参数，优先借用而不是先 clone。

## slice 借用集合的一部分

`&str` 是字符串 slice，`&[T]` 是序列 slice。slice 通常由指针和长度组成，不拥有底层数据：

```rust
fn first_two(values: &[u32]) -> &[u32] {
    values.get(..2).unwrap_or(values)
}
```

这里 `get(..2)` 安全处理短数组，返回 slice 的生命周期来自输入。底层 Vec 重新分配或 String 被修改
时，原 slice 不能继续有效，因此编译器禁止同时持有冲突借用。

## 生命周期参数描述引用之间的关系

大多数生命周期由编译器省略规则推断。函数可能返回两个输入之一时，需要明确它们共享的约束：

```rust
pub fn first_non_empty<'a>(primary: &'a str, fallback: &'a str) -> &'a str {
    if primary.trim().is_empty() { fallback } else { primary }
}
```

`'a` 不是“让值活得更久”，而是告诉编译器：返回引用不会超过两个输入引用都有效的范围。

生命周期标注也不改变运行时行为，不会延长变量作用域、分配内存或插入引用计数。

## 生命周期省略规则

编译器能推断常见函数签名：

```rust
fn first_word(input: &str) -> &str
```

概念上等价于输入和输出共享一个生命周期。方法有 `&self` 时，输出引用通常默认绑定到 self：

```rust
fn name(&self) -> &str
```

多个输入引用且返回关系不明确时才需要显式参数。不要为了显得严谨给每个引用都加 `'a`；冗余标注
会让真正关系更难看见。

## 在 struct 中保存引用

```rust
struct PromptView<'a> {
    text: &'a str,
}
```

类型参数 `'a` 表示 PromptView 不能比 text 活得更久。长期存储、跨任务传递或所有权边界不清时，
保存 String 或 Arc<str> 往往比让整个对象图携带生命周期更实用。引用字段适合明确、短期的解析视图。

## `'static` 的两个含义

- `&'static str`：引用内容存在整个程序期间，字符串字面量属于这种情况。
- `T: 'static`：T 不包含借用的短生命周期引用；拥有的 String、Vec 通常满足，即使值稍后会 drop。

`tokio::spawn` 的 `'static` 约束是后者：后台任务不能借用即将返回的栈帧，不是要求任务永远运行。

## trait 方法里的生命周期

框架的异步 trait 经常出现：

```rust
fn execute<'a>(&'a self, task: &'a str) -> BoxFuture<'a, Result<String>>;
```

返回 Future 可能借用 `self` 和 `task`，因此 Future 的生命周期与输入借用绑定。第 11、12 章会继续
解释 `BoxFuture` 和异步 trait object。

## 所有权与并发

`tokio::spawn` 创建的任务可能比当前函数活得更久，捕获值通常必须是 `'static`。这里的
`'static` 表示任务不借用当前栈帧，不代表值必须永久存在。常用做法是把拥有所有权的 `String`、
`Arc<T>` 或取消令牌 move 进任务。`async move` 取得捕获值所有权，但不会自动 clone；同一个 Arc 要
给多个任务时，每个任务需要自己的 `Arc::clone`。

## 常见借用错误怎么读

| 编译器信息 | 真正含义 | 优先检查 |
|------------|----------|----------|
| borrow of moved value | 值已经被传参、赋值或 into_iter 消耗 | 调用是否应借用？之后是否还需要原值？ |
| cannot borrow as mutable | 仍有只读借用或绑定非 mut | 缩短只读借用，确认是否真需修改 |
| borrowed value does not live long enough | 返回值/任务活得比来源久 | 改为拥有值，或调整真正所有者 |
| cannot return reference to local variable | 函数结束后局部值会 drop | 返回 String/Arc 等拥有类型 |
| lifetime may not live long enough | 签名没有表达引用关系 | 标明输入输出关系，不凭空加 `'static` |

## 运行示例

```bash
cargo run -p echo_rust_learning --example chapter_04_ownership
cargo test -p echo_rust_learning ownership
```

## 项目映射

- [`echo-core/src/tools/mod.rs`](../../../echo-core/src/tools/mod.rs)：trait 方法显式生命周期。
- [`echo-state/src/memory/store.rs`](../../../echo-state/src/memory/store.rs)：借用 namespace、拥有存储值。
- [`src/agent/react/run/stream_channel.rs`](../../../src/agent/react/run/stream_channel.rs)：把拥有的运行数据移入异步任务。

## 练习

1. 把一个接收 `String` 但只读的函数改为接收 `&str`，观察调用点减少了哪些 clone。
2. 写一个从两个 `LearningTask` 返回较短标题的函数，标注正确生命周期。
3. 解释为什么不能从函数返回指向函数内部临时 `String` 的 `&str`。
4. 写一个包含 `&str` 的 `PromptView<'a>`，观察它为何不能离开原 String 的作用域。
5. 找出一个不必要 clone，分别尝试借用和转移所有权，比较 API 意图。

下一章：[集合、闭包与迭代器](05-collections-closures-iterators.md)。
