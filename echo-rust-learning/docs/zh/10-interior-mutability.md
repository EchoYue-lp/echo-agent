# 10 内部可变性：Cell、RefCell、锁与原子类型

普通借用规则要求：多个 `&T` 可以共存，而 `&mut T` 必须独占。内部可变性允许代码通过共享引用
修改状态，但没有取消这条规则，只是把检查交给类型自己的安全协议：`RefCell` 在运行时检查，锁在
线程间协调，原子类型由 CPU 原子操作保证。

```bash
cargo run -p echo_rust_learning --example chapter_10_rc_refcell
```

所有安全内部可变性类型的底层原语都是 `UnsafeCell<T>`。业务代码通常不直接使用它；标准库和
Tokio 已把不安全细节封装进安全 API。

## Cell：整体读写小值

`Cell<T>` 适合 `Copy` 小值，不返回指向内部的引用，而是整体 get/set：

```rust
use std::cell::Cell;

let attempts = Cell::new(0_u32);
attempts.set(attempts.get().saturating_add(1));
assert_eq!(attempts.get(), 1);
```

它适合单线程计数、缓存标志等局部实现。`Cell` 不是 `Sync`，不能作为多线程共享计数器。对于不
实现 `Copy` 的值，可以用 `replace`、`take` 或 `into_inner` 整体移入移出。

## RefCell：把借用规则移到运行时

`RefCell<T>` 允许通过 `&RefCell<T>` 取得读 guard `Ref<T>` 或写 guard `RefMut<T>`：同一时刻仍然
只能“多个读”或“一个写”。区别是冲突不再是编译错误，而是运行时结果。

教学代码使用可恢复 API：

```rust
let mut entries = cell
    .try_borrow_mut()
    .map_err(|_| LearningError::BorrowConflict)?;
entries.push(value);
```

`borrow()` 和 `borrow_mut()` 在冲突时会 panic，本项目代码不使用这种假定。完整示例中的
`demonstrate_conflict()` 故意保持一个读 guard，再尝试写借用，将冲突转换为领域错误。

guard 的作用域决定借用持续多久。可用内层 block 明确提前释放：

```rust
{
    let view = cell.try_borrow().map_err(|_| LearningError::BorrowConflict)?;
    render(&view);
} // view 在这里 drop，后面可以取得写借用
```

`Rc<RefCell<T>>` 是单线程共享可变状态的常见组合：`Rc` 负责多个所有者，`RefCell` 负责修改协议。
它适合树节点回指、单线程 UI 等场景，但借用关系复杂后会把问题推迟到运行时。能重新设计所有权或
用消息传递表达时，优先降低共享写入。

## Mutex 与 RwLock

线程安全共享状态通常配合 `Arc`：

```text
Arc<T>             多个所有者，只读共享
Arc<Mutex<T>>      多个所有者，一个访问者进入临界区
Arc<RwLock<T>>     多个所有者，多个读者或一个写者
```

`MutexGuard`/`RwLockGuard` 和 `RefCell` guard 类似，离开作用域时自动释放。`RwLock` 只有在确实
读多写少、测量后有收益时才优于 `Mutex`；它的实现和公平策略更复杂，写者也可能等待更久。

标准库锁会报告 poisoning：某线程持锁时 panic，锁内数据可能只更新了一半。调用方应把
`PoisonError` 转换为错误或采用明确恢复策略，不能直接强制提取。

## std::sync 还是 tokio::sync

| 情况 | 优先选择 |
|------|----------|
| 临界区很短，获取后不 `.await` | `std::sync::Mutex/RwLock` |
| 等锁时需要让出运行时线程 | `tokio::sync::Mutex/RwLock` |
| 只更新独立数字或标志 | 原子类型 |
| 状态能归一个任务所有 | channel |

不要持有同步锁 guard 跨 `.await`。任务暂停时 guard 不会自动释放，其他任务可能一直堵塞，Future
也可能因此不满足 `Send`。通常先在临界区 clone 所需快照，离开作用域释放 guard，再调用异步代码。

教学代码的 [`SharedProgress`](../../src/smart_pointers/synchronization.rs) 使用
`Arc<tokio::sync::RwLock<HashMap<...>>>`。`Arc` 和锁职责不同，缺一不能推出另一项能力。

## 原子类型与内存顺序

原子类型适合单个计数器或标志：

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

let progress = AtomicUsize::new(0);
progress.store(50, Ordering::Release);
let observed = progress.load(Ordering::Acquire);
assert_eq!(observed, 50);
```

[`AtomicProgress`](../../src/smart_pointers/synchronization.rs) 展示了相同模式。
内存顺序定义原子操作与其他内存访问的可见性关系：

- `Relaxed` 只保证这个原子值自身操作不可分割，适合纯统计计数。
- `Release` 发布此前写入，`Acquire` 观察对应发布后的状态。
- `AcqRel` 用于同时读改写。
- `SeqCst` 提供最强的全局顺序，也更容易理解但可能限制优化。

不要凭直觉选择最弱顺序。若计数本身就是全部状态，`Relaxed` 常已足够；若标志表示“其他数据已
准备好”，通常需要 Acquire/Release 协议，并应写测试和注释说明不变量。

## OnceLock 与 LazyLock

`OnceLock<T>` 线程安全地初始化一次，之后提供只读引用；`LazyLock<T>` 在首次访问时运行初始化
闭包。它们适合不可变注册表或缓存，不适合需要重载的运行时配置。

初始化可能失败时，优先让外层函数返回 `Result`，成功构造后再调用 `set`。不要把可恢复错误藏在
会终止程序的初始化路径里。

## channel 往往比锁更清晰

如果只有一个任务真正负责状态，其他任务可以发送 `UpdateProgress`、`Cancel` 等消息。这样状态
不需要锁，更新顺序也集中在一个事件循环中。代价是必须设计 channel 容量、背压、关闭和响应协议。
第 12 章会详细比较不同 channel。

## 死锁与锁范围

两个任务以相反顺序取得 A、B 两把锁会死锁。降低风险的方法：

1. 固定全局锁顺序。
2. 缩小 guard 作用域，不在锁内执行回调、IO 或未知代码。
3. 不把锁 guard 存进长生命周期结构。
4. 能合并为一个一致状态时，不拆成多把互相依赖的锁。
5. 用 channel 把写所有权集中到单个任务。

## 项目映射

- [`SharedState`](../../../echo-orchestration/src/workflow/state.rs)：异步工作流共享状态。
- [`InMemoryStore`](../../../echo-state/src/memory/store.rs)：锁保护内存存储。
- [`circuit_breaker.rs`](../../../echo-core/src/circuit_breaker.rs)：共享许可状态。
- [`synchronization.rs`](../../src/smart_pointers/synchronization.rs)：锁和原子教学实现。

## 练习

1. 运行 `chapter_10_rc_refcell`，解释读 guard 为什么会阻止写 guard。
2. 给 `SharedProgress` 增加只返回已完成任务的方法，确保锁内不执行异步回调。
3. 将纯次数统计改为 `AtomicUsize`，分别解释 `Relaxed` 与 Acquire/Release 是否适用。
4. 设计一个 channel 版本的进度管理器，对比它与 `Arc<RwLock<_>>` 的所有权模型。

下一章：[Cow、Pin 与 Future](11-cow-pin-futures.md)。
