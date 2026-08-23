# 11 Cow、Pin、Future 与 Stream 底层

这一章连接所有权和异步运行时：`Cow` 在借用与拥有之间延迟分配，Future 把异步函数编译为状态机，
`Pin` 则限制状态机在开始执行后的移动方式。先运行示例，再带着输出阅读类型签名：

```bash
cargo run -p echo_rust_learning --example chapter_11_pin_future
```

## Cow：多数只读、少数修改

`Cow<'a, T>` 是 clone-on-write，包含 `Borrowed(&'a T)` 或 `Owned(T::Owned)` 两种状态。文本处理中
常写作 `Cow<'a, str>`：

```rust
use std::borrow::Cow;

pub fn normalize_prompt(input: &str) -> Cow<'_, str> {
    let trimmed = input.trim();
    if trimmed == input {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(trimmed.to_string())
    }
}
```

输入本来规范时零分配；需要去除空白时才创建 `String`。调用方可通过 `as_ref()` 统一获得 `&str`，
最终需要所有权时调用 `into_owned()`。

`to_mut()` 会在 Borrowed 状态首次复制，然后返回 `&mut`；已经 Owned 时直接修改。教学函数
[`ensure_period`](../../src/smart_pointers/cow.rs) 展示了这一过程。

不要为了“可能省一次分配”到处使用 Cow。如果调用方最终总要 `String`，直接返回 `String` 更清晰；
如果分支很少或文本很短，先以 API 易懂为主，再根据测量优化。

## 调用 async fn 时发生了什么

调用普通函数会立即执行函数体。调用 `async fn` 只构造一个 Future，直到它被 `.await`、spawn 或
交给执行器 poll 才开始推进：

```rust
let future = load_profile(); // 此处尚未完成 IO
let profile = future.await?;
```

编译器大致把 async 函数转为 enum 状态机，每个 `.await` 点对应一种暂停状态，并保存恢复执行所需
的局部变量。状态机实现：

```rust
trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}
```

- `Poll::Ready(value)` 表示完成，通常不能再 poll。
- `Poll::Pending` 表示暂时不能继续；Future 必须安排 waker 在条件变化时通知执行器。

Future 是惰性的。创建后既不 await 也不 spawn，它就不会完成预期工作。

## Waker 不是忙循环

执行器 poll 到 Pending 后会去运行其他任务。socket 可读、计时器到期或 channel 收到消息时，资源
驱动调用 waker，执行器才再次 poll。正确 Future 不应在 Pending 时不断自唤醒做无意义轮询，否则
会造成高 CPU 占用。

业务开发通常使用 Tokio 提供的 Future，不需要手写 poll；理解它有助于排查“为什么任务没有被
唤醒”以及自定义 Stream 的实现。

## 为什么 Future 会涉及 Pin

异步状态机可能在一个字段中保存数据，并在另一个字段中保存指向该数据的引用。开始 poll 后若整体
移动，内部地址关系可能失效。`Pin<P>` 的核心保证是：只要通过这个 pinned 指针访问，就不能再以
违反类型约束的方式移动内部值。

需要避免三个误解：

1. `Pin` 不等于堆分配；`Pin<&mut T>` 也能固定借用值。
2. `Box<T>` 地址稳定通常很方便，但只有 `Pin<Box<T>>` 把不移动保证写入类型。
3. `Pin` 不是“对象永远不能 move”；实现 `Unpin` 的普通类型即使被 Pin 也可安全移动。

大多数普通结构自动实现 `Unpin`。编译器生成的部分 Future 可能不实现它，所以通用 Future API 以
`Pin<&mut Self>` 接收自身。学习代码不需要使用 `unsafe` 手写自引用结构。

## BoxFuture 拆解

`futures::future::BoxFuture<'a, T>` 近似于：

```rust
Pin<Box<dyn Future<Output = T> + Send + 'a>>
```

从内到外阅读：

1. `Future<Output = T>` 规定完成值。
2. `dyn` 擦除具体状态机类型，允许不同分支返回同一类型。
3. `Send` 允许多线程执行器在线程间调度它。
4. `'a` 表示 Future 内部借用至少在这段生命周期内有效。
5. `Box` 提供固定大小、拥有的动态值。
6. `Pin` 保证 poll 期间不发生非法移动。

`'static` 不表示 Future 永远存活，只表示它不借用短生命周期局部值。`async move` 可以把拥有值
移入状态机，使其更容易满足 `'static`，但不应因此无条件 clone 大对象。

## impl Future 与 BoxFuture

```rust
fn concrete() -> impl Future<Output = String> { async { String::from("ok") } }
fn erased() -> BoxFuture<'static, String> { Box::pin(async { String::from("ok") }) }
```

`impl Future` 保留静态分发，通常无堆分配，适合函数返回单一具体实现。BoxFuture 适合 trait 方法、
异构分支、递归 async 或需要存入集合的 Future，代价是分配和动态分发。先选能表达需求的最简单
类型，不要习惯性 Box 每个 Future。

## 递归 async 为什么要装箱

普通递归函数每次调用只在运行时增加栈帧，函数类型本身大小固定。递归 async 状态机若直接包含
下一层同类型 Future，会形成无限大小：

```text
Future = 当前状态 + Future = 当前状态 + 当前状态 + Future + ...
```

`Box::pin` 把递归边改为固定大小指针，从而打断无限类型。项目的递归工作流执行路径采用这一模式。

## Stream 是多次产生值的 Future

Iterator 的 `next()` 同步返回下一项；Stream 的 `poll_next()` 可以 Pending：

```rust
Pin<Box<dyn Stream<Item = Result<Event, Error>> + Send + 'a>>
```

动态 Stream 同样组合 Box、dyn、Pin、Send 和生命周期。消费时 `StreamExt::next().await`，并分别
处理“产生事件”“业务错误”“流正常结束”。流结束不是错误，取消也不应伪装成普通文本事件。

## 常见编译错误怎么读

- `future cannot be sent between threads safely`：检查哪个值跨 `.await` 存活且不是 Send，常见是
  `Rc`、`RefCell` 或同步锁 guard。
- `borrowed value does not live long enough`：Future 保存了局部引用，却被要求活得更久。
- `cannot be unpinned`：调用 API 要求 Unpin；可在调用处 pin Future，而不是随意修改类型约束。
- 不同 async block 类型不一致：每个 async block 都有匿名独特类型，可用 BoxFuture 擦除或调整
  控制流让其共享一个 block。

## 项目映射

- [`Tool::execute`](../../../echo-core/src/tools/mod.rs)：带借用生命周期的 BoxFuture。
- [`sandbox/manager.rs`](../../../echo-execution/src/sandbox/manager.rs)：动态 Stream。
- [`workflow/node.rs`](../../../echo-orchestration/src/workflow/node.rs)：递归 async 图执行。
- [`pinning.rs`](../../src/smart_pointers/pinning.rs)：最小 BoxFuture 示例。

## 练习

1. 为 `normalize_prompt` 与 `ensure_period` 分别断言 Borrowed 和 Owned 路径。
2. 修改 `boxed_message` 接受 `&str`，解释返回值为什么不能继续声明为 `'static`。
3. 写出 `BoxFuture` 每一层类型所解决的问题，不看上文复述。
4. 写一个产生三个进度值的 Stream，用 `StreamExt::next` 消费并处理结束。

下一章：[Tokio、并发、channel 与取消](12-async-concurrency-streams.md)。
