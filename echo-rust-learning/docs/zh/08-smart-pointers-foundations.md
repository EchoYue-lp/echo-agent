# 08 智能指针基础：Box、Deref、Drop 与 RAII

普通引用 `&T` 只借用值，不拥有值。智能指针是拥有数据或附带额外行为的数据结构，通常实现
`Deref`、`Drop`，有时还实现引用计数、内部可变性或固定地址语义。

## 什么叫“指针”和“智能”

指针保存另一个内存位置。普通引用 `&T` 带有编译期借用规则，但不拥有目标。智能指针通常是
struct，通过 trait 提供额外语义：

- `Deref`：像引用一样访问内部值。
- `Drop`：离开作用域时自动清理。
- 所有权：Box 独占，Rc/Arc 共享，Weak 不保持存活。
- 可变性/同步：RefCell、Mutex、RwLock 在运行时协调访问。

先运行：

```bash
cargo run -p echo_rust_learning --example chapter_08_box
```

## 栈、堆与 `Box<T>`

栈上的值必须在编译期知道大小。`Box<T>` 本身大小固定，内部的 `T` 存在堆上：

```rust
let value = Box::new(String::from("heap value"));
println!("{}", value.len());
```

这里调用 `len()` 时发生了自动解引用。`Box<T>` 实现 `Deref<Target = T>`，所以大多数只读操作
看起来和直接使用 `T` 一样。

把值放进 Box 会移动所有权：

```rust
let boxed = Box::new(String::from("prompt"));
let moved = boxed;
// boxed 不再可用，moved 是唯一所有者
```

Box clone 会调用内部 T 的 Clone，通常产生另一份独立堆数据，不是共享指针。需要共享所有权时看
第 9 章。

不要为了“对象很大”就机械使用 Box。Box 的核心价值是：

- 给递归类型一个确定大小。
- 把大值或 trait object 放到堆上并转移所有权。
- 配合 `Pin` 固定值的位置。
- 缩小大 enum 变体，控制公共返回类型大小。

## Sized 与动态大小类型

普通泛型参数隐含 `T: Sized`。`str`、`[T]`、`dyn Trait` 在编译期没有独立确定大小，不能直接作为
局部值或普通字段，但能放在 `&str`、`Box<[T]>`、`Box<dyn Trait>` 等固定大小指针后面。

`?Sized` 放宽默认 Sized 约束：

```rust
fn inspect<T: ?Sized>(value: &T) {
    // 只持有固定大小的引用
}
```

不是所有类型都应改成 `?Sized`。只有 API 确实需要接受 slice、str 或 trait object 时才增加复杂度。

## 递归类型为什么需要 Box

下面的递归定义无法确定大小：一个节点里面继续完整包含两个节点，编译器会无限展开。把子节点
放进 Box 后，父节点只保存两个固定大小的指针：

```rust
pub enum PlanNode {
    Step(String),
    Sequence(Box<PlanNode>, Box<PlanNode>),
}
```

完整代码见
[`box_pointer.rs`](../../src/smart_pointers/box_pointer.rs)。运行：

```bash
cargo run -p echo_rust_learning --example chapter_08_box
```

递归结构仍需考虑深度：Box 解决类型大小，不自动防止极深递归导致调用栈耗尽。处理不可信的递归
数据时要限制深度或改用显式 Vec 栈。

## `Box<dyn Trait>` 与动态分发

不同类型实现同一个 trait，但大小可能不同。`dyn Trait` 本身是动态大小类型，通常通过胖指针
访问。胖指针包含数据地址和虚函数表地址：

```rust
let formatter: Box<dyn TaskFormatter> = Box::new(PlainFormatter);
```

编译器在运行时通过虚函数表选择具体实现。这允许 `Vec<Box<dyn Tool>>` 同时保存文件工具、
搜索工具和 Subagent 调度工具。

对比两种分发：

| 形式 | 分发方式 | 特点 |
|------|----------|------|
| `fn run<T: Tool>(tool: &T)` | 静态分发 | 可内联，类型在编译期确定 |
| `fn run(tool: &dyn Tool)` | 动态分发 | 可混合不同实现，存在间接调用 |
| `Box<dyn Tool>` | 动态分发 + 所有权 | 适合容器或结构体字段 |
| `Arc<dyn Tool>` | 动态分发 + 共享所有权 | 适合跨任务共享 |

`Box<dyn Trait>` 指向的数据区大小取决于具体实现；另一部分 vtable 指针让程序知道如何调用方法和
析构具体值。Box 被 drop 时会通过 vtable 调用正确实现的 Drop。

## Deref 与自动解引用

教学 [`PromptBox`](../../src/smart_pointers/box_pointer.rs) 包装 `Box<str>`：

```rust
impl Deref for PromptBox {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
```

因此 `prompt.len()` 可以调用 str 方法，`&PromptBox` 也可在特定函数参数处 coercion 为 `&str`。

不要滥用 Deref 模拟面向对象继承。Deref 的调用是隐式的，适合真正“像指针”的包装；普通领域
包装优先提供具名方法或实现 `AsRef<T>`/`Borrow<T>`。

`DerefMut` 允许通过 `&mut Wrapper` 修改 Target，但仍受唯一可变借用规则限制。

## RAII 与 Drop

RAII 表示资源生命周期绑定到值的生命周期。值离开作用域时，`Drop::drop` 自动运行：

- `String` 释放堆缓冲区。
- `File` 关闭文件描述符。
- 锁 guard 释放锁。
- `Arc` 减少强引用计数。
- 管理中的流在 Drop 中发出取消并回收后台任务。

通常不要手动调用 `Drop::drop`。需要提前结束资源生命周期时使用 `drop(value)`，让所有权在该点
被消耗。

教学 [`DropCounter`](../../src/smart_pointers/box_pointer.rs) 用 AtomicUsize 让
析构发生的时机可以确定性测试。真实代码不应让 Drop 执行复杂异步工作，因为 Drop 不能 await，
也不应在 Drop 中悄悄忽略关键持久化错误。

### Drop 顺序

- 局部变量通常按创建的逆序 drop。
- struct 字段按声明顺序 drop。
- Vec 会 drop 每个元素，再释放缓冲区。
- panic unwind 时也可能 drop 已初始化值，但进程 abort 时不保证。

不能把正确性建立在进程退出一定运行析构上。重要数据使用显式 `flush`/`shutdown` Result，再让 Drop
只负责兜底释放内存和系统句柄。

### Guard 是 RAII 的典型应用

锁的 `lock()` 返回 guard。guard 的 Deref 提供内部数据访问，Drop 释放锁。想在 await 前解锁时，
缩小 guard 作用域或显式 `drop(guard)`；不要只 drop 内部引用。

## 框架里的真实用法

- [`Tool` trait](../../../echo-core/src/tools/mod.rs)：`Box<dyn Tool>` 保存异构工具。
- [`ReactError`](../../../echo-core/src/error.rs)：Box 包装子错误，限制 enum 大小。
- [`workflow/node.rs`](../../../echo-orchestration/src/workflow/node.rs)：Box 保存子图和节点函数。
- [`stream_channel.rs`](../../../src/agent/react/run/stream_channel.rs)：Drop 中取消流任务。

## 选择建议

如果只有一个所有者且不需要递归或类型擦除，直接保存 `T`。如果需要独占堆所有权或动态 trait，
使用 `Box<T>`/`Box<dyn Trait>`。需要多个所有者时再进入下一章的 `Rc` 和 `Arc`。

选择清单：

| 需求 | 类型 |
|------|------|
| 一个所有者、直接保存即可 | `T` |
| 一个所有者、递归/DST/trait object | `Box<T>` / `Box<dyn Trait>` |
| 单线程多个所有者 | `Rc<T>` |
| 多线程多个所有者 | `Arc<T>` |
| 不应延长生命周期的反向引用 | `Weak<T>` |
| 只借用、不拥有 | `&T` / `&mut T` |

## 练习

1. 为 `PlanNode` 增加 `Parallel(Vec<PlanNode>)`，比较它为什么不需要为每个元素手写 Box。
2. 写两个 `TaskFormatter` 实现并放入同一个 `Vec<Box<dyn TaskFormatter>>`。
3. 在源码中找出一个“为了缩小 enum”而使用 Box 的例子，解释最大变体如何影响 enum 大小。
4. 给 PromptBox 实现 AsRef<str>，比较它与 Deref coercion 的显式程度。
5. 用 DropCounter 验证嵌套作用域的逆序释放，不依赖 println 做断言。

下一章：[Rc、Arc 与 Weak](09-shared-ownership.md)。
