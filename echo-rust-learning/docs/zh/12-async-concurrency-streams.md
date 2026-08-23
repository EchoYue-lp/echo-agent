# 12 Tokio、并发、channel、超时与取消

Agent 会同时等待模型流、工具进程、Subagent 结果、用户输入和取消信号。Rust async 把等待过程
表示为 Future；Tokio 提供执行器、异步 IO、计时器、任务和 channel。本章关注如何组合这些原语，
而不是把“能同时运行”误认为“自动正确”。

```bash
cargo run -p echo_rust_learning --example chapter_12_async_concurrency
```

## 顺序、并发与并行

```rust
let first = load_first().await?;
let second = load_second().await?;
```

这两步是顺序执行。第一个等待时当前任务会让出线程，但第二个 Future 还没开始。需要并发时，可
按语义选择：

- `tokio::join!`：固定数量任务都要完成。
- `tokio::try_join!`：任一失败就尽早返回错误。
- `FuturesUnordered`：动态数量任务，按完成顺序消费。
- `tokio::spawn`：创建可独立调度和取消的任务。
- `tokio::select!`：多个异步事件竞争，处理先完成的分支。

并发是多个任务交错推进；并行是在多个 CPU 核心同时计算。异步主要解决等待型 IO。CPU 密集或
阻塞调用长期占用 Tokio 核心线程时，应使用 `spawn_blocking` 或专门执行器。

## spawn、所有权与两层错误

```rust
let handle = tokio::spawn(async move {
    run_subagent(name).await
});
```

多线程 runtime 上 spawn 的 Future 通常要求 `Send + 'static`。`async move` 把捕获值的所有权移入
状态机，因此局部引用不会悬空；但它并不会让 `Rc` 等非 Send 类型自动变为 Send。

等待 `JoinHandle` 常有两层结果：

```text
Result<              JoinError,   任务 panic 或被 abort
    Result<T, E>                  业务执行成功或失败
>
```

教学函数 [`run_subagents`](../../src/async_concurrency.rs) 分别把 JoinError 与
channel 错误转换为 `LearningError`。不要把任务调度失败和业务失败压成同一个无结构字符串。

## 结构化并发

启动任务的作用域也应负责等待、取消或收集它们。若函数 spawn 后立即返回且丢失 handle，后台任务
可能比请求活得更久，错误也无人观察。`run_subagents` 保存每个 handle，等待完毕后再收集结果，
体现了最小的结构化并发边界。

对并发 Agent 系统还应明确：

- 哪个父任务拥有子任务。
- 父任务失败时子任务是否取消。
- 所有子任务结束前谁保持资源存活。
- 错误是 fail-fast，还是收集全部结果后汇总。

## 有界 mpsc 与背压

`mpsc::channel(capacity)` 是多生产者、单消费者队列。容量满时 `send().await` 挂起生产者，直到
消费者腾出空间，这就是背压。无界队列虽然简单，却可能在模型或工具产生速度高于 UI 消费速度时
持续占用内存。

```rust
let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
sender
    .send(event)
    .await
    .map_err(|_| LearningError::ChannelClosed)?;
drop(sender);

while let Some(event) = receiver.recv().await {
    handle(event).await?;
}
```

所有 sender 都 drop 后，receiver 才读到 `None`。忘记释放最后一个 sender 是消费循环永不结束的
常见原因。容量不是随意数字，应结合允许的突发量、单条消息大小和响应延迟选择。

## 四类常用 channel

| 类型 | 语义 | 典型用途 |
|------|------|----------|
| `mpsc` | 多生产者排队给一个消费者 | 事件循环、工具结果 |
| `oneshot` | 单次发送、单次响应 | 请求与回复 |
| `watch` | 只保留最新值 | 取消标志、配置快照 |
| `broadcast` | 每个订阅者都接收后续消息 | 多 UI/channel 事件投递 |

`watch` 不保存全部历史，慢消费者只观察最新状态；`broadcast` 的慢消费者可能落后并收到 lag 错误。
按业务语义选择，不要把所有通知都塞入 mpsc。

## select 与取消安全

教学函数 `wait_or_cancel` 同时等待睡眠和 watch 取消信号：

```rust
tokio::select! {
    _ = tokio::time::sleep(delay) => Ok("completed"),
    changed = cancelled.changed() => {
        changed.map_err(|_| LearningError::ChannelClosed)?;
        if *cancelled.borrow() {
            Err(LearningError::Cancelled)
        } else {
            Ok("cancel signal cleared")
        }
    }
}
```

select 选中一个分支后，未选 Future 通常会被 drop。一个操作若在被 drop 后会丢失已经读取但尚未
处理的数据，就不是 cancellation-safe。使用 select 前应查 API 文档，或把不可取消的关键步骤放入
独立受管理任务。

取消是正常控制流，不等于失败。业务错误、用户取消、父任务结束、超时和强制 abort 应保持不同
类型，UI 和清理策略才有依据。

## 超时包住什么边界

```rust
tokio::time::timeout(deadline, operation()).await
```

超时会在期限到达时 drop 内层 Future，并返回 elapsed 错误。它不保证底层外部操作一定停止：例如
已启动的进程、独立 spawn 的任务或远端请求可能继续运行。设计超时时要明确：

1. 内层 Future 被 drop 是否能完成清理。
2. 是否需要向外部进程或子任务发送取消。
3. 超时错误应记录哪个阶段和期限。
4. 重试是否会造成重复副作用。

[`complete_within`](../../src/async_concurrency.rs) 与 `wait_or_cancel` 刻意返回不同
错误，测试也验证二者不会混淆。

## Stream：异步事件序列

Stream 类似异步 Iterator。消费时要处理 item、item 内部错误和正常结束：

```rust
while let Some(event) = stream.next().await {
    let event = event?;
    handle(event).await?;
}
```

流协议应定义终止、取消和失败事件，不能靠某段文本猜状态。消费者提前 drop Stream 时，生产任务
是否同步取消也必须明确；否则后台模型或工具调用可能继续占资源。

## 锁、await 与 Send

不要持有同步锁 guard 跨越 `.await`。除死锁风险外，guard 还可能使整个 Future 不满足 Send，
导致 spawn 编译失败。把临界区限制在一个 block 内，复制必要快照后再 await。异步锁允许等待锁时
让出线程，但也不代表适合在锁内做网络 IO。

## 可测试的异步代码

异步测试使用 `#[tokio::test]`。避免依赖任务输出顺序，因为调度顺序不是协议。测试应断言集合、
状态和明确事件。计时测试使用明显分离的时长，并尽量使用 Tokio 的暂停时间能力，避免 CI 抖动。

## 项目映射

- [`stream_channel.rs`](../../../src/agent/react/run/stream_channel.rs)：Stream、channel、Drop 与取消。
- [`echo-execution/src/tools.rs`](../../../echo-execution/src/tools.rs)：并发工具批次和超时。
- [`echo-orchestration/src/tasks`](../../../echo-orchestration/src/tasks)：任务图与进度事件。
- [`echo-core/src/agent/mod.rs`](../../../echo-core/src/agent/mod.rs)：AgentEvent 流协议。

## 练习

1. 把教学 mpsc 容量改为 1，加入消费者延迟，观察发送端背压。
2. 用 `FuturesUnordered` 收集 Subagent 结果，保持调度错误和业务错误分离。
3. 为取消测试增加“信号初始值已为 true”的路径。
4. 列出一个工具超时后仍需清理的外部资源，并设计拥有者。

下一章：[Serde、JSON 与配置边界](13-serde-and-configuration.md)。
