# 09 共享所有权：Rc、Arc 与 Weak

普通引用 `&T` 只是暂时借用，生命周期不能超过所有者。如果对象图、注册表或异步任务都要独立
持有同一个值，就需要明确的共享所有权。`Rc<T>` 与 `Arc<T>` 用引用计数记录强所有者数量，最后
一个强所有者离开作用域时才销毁内部值。

先运行多线程版本：

```bash
cargo run -p echo_agent_learning --example chapter_09_arc_weak
```

## 引用计数解决了什么

下面的 `String` 只有一个所有者，move 给第一个任务后，当前作用域就不能再把它交给第二个任务：

```rust
let name = String::from("reviewer");
consume(name);
// name 已被 move
```

共享指针把“谁负责释放”改为“最后一个强所有者负责释放”：

```rust
use std::sync::Arc;

let first = Arc::new(String::from("reviewer"));
let second = Arc::clone(&first);

assert_eq!(Arc::strong_count(&first), 2);
drop(second);
assert_eq!(Arc::strong_count(&first), 1);
```

`Arc::clone` 只增加计数，不深拷贝 `String`。显式写 `Arc::clone(&value)` 比 `value.clone()` 更容易
让读者看出这里发生的是共享所有权操作。计数只能用于诊断，不应把瞬时 count 当作并发业务判断。

## Rc 与 Arc 的选择

| 类型 | 计数方式 | 能否跨线程 | 常见场景 |
|------|----------|------------|----------|
| `Rc<T>` | 非原子，开销较低 | 否 | 单线程树、图、UI 状态 |
| `Arc<T>` | 原子操作 | 取决于 `T` | Tokio 任务、线程、框架服务 |

`Rc` 不实现 `Send` 和 `Sync`。这不是功能缺失，而是它的计数操作没有线程同步；编译器因此会拒绝
把 `Rc<T>` move 进多线程 `tokio::spawn`。不要为了绕过错误盲目换成 `Arc`，先确认代码是否真的
需要跨线程共享。

## Arc 只共享所有权，不提供可变性

`Arc<T>` 通常只能得到 `&T`。要修改内部值，需要另外选择同步策略：

- 简单计数或标志：`AtomicUsize`、`AtomicBool`。
- 短临界区：`Mutex<T>` 或 `RwLock<T>`。
- 状态可以由一个任务独占：优先通过 channel 发送命令。
- 初始化后只读：`OnceLock<T>` 或在创建 `Arc` 前完成构造。

如果当前只有一个强引用，`Arc::get_mut(&mut arc)` 可以安全返回 `&mut T`；一旦还有其他强引用，
它返回 `None`。`Arc::make_mut` 则实现 copy-on-write：共享时先克隆，唯一时原地修改。它要求
`T: Clone`，适合“读多、偶尔生成新版本”的快照，不适合频繁共享写入。

## Weak 是不拥有对象的观察者

`Weak<T>` 与强引用指向同一控制块，但不会让内部值继续存活：

```rust
let agent = Arc::new(String::from("planner"));
let observer = Arc::downgrade(&agent);

let before_drop = observer.upgrade(); // Some(Arc<String>)
drop(before_drop);
drop(agent);
let after_drop = observer.upgrade();  // None
```

`upgrade()` 必须返回 `Option<Arc<T>>`，因为检查之前与使用时之间对象可能已经被其他线程释放。
拿到 `Some(Arc<T>)` 后，这个新强引用会在当前操作期间保持对象存活。

教学代码的 [`AgentRegistry`](../../src/smart_pointers/arc_weak.rs) 只保存 Weak。
注册表能找到活跃 Agent，却不会因为“曾经注册”而永久延长其生命周期。`counts()` 还展示强弱计数，
便于观察 `Arc` 控制块的行为。

## 引用环为何不会自动回收

如果父节点用强引用持有子节点，子节点又用强引用持有父节点，即使外部都已释放，两边强计数仍然
大于零。Rust 仍然内存安全，但值的 `Drop` 不会运行，内存和文件句柄等资源会泄漏。

常见所有权方向：

```text
所有者 / 父节点  --Arc/Rc-->  被拥有值 / 子节点
观察者 / 子节点  --Weak---->  所有者 / 父节点
```

设计对象图时先问“谁决定它何时销毁”。决定生命周期的一侧用强引用，只需导航或回调的一侧用
Weak。缓存、观察者列表和注册表通常也是 Weak 的合适边界。

## Send 与 Sync 如何传播

- `Send`：值的所有权可以安全地移动到另一个线程。
- `Sync`：`&T` 可以安全地被多个线程共享。

`Arc<T>` 只有在 `T: Send + Sync` 时才适合在线程间共享。`Arc<RefCell<T>>` 仍然不能变成线程安全，
因为 `RefCell<T>` 不是 `Sync`。编译器会递归检查所有字段，错误信息中的长类型链通常就是在指出
哪一个内部类型破坏了约束。

`dyn Trait + Send + Sync` 把这些约束写到 trait object 上。echo-agent 的工具、模型客户端和 Store
常被多个异步任务共享，因此公共契约会明确要求这两个 marker trait。

## 成本与常见误区

1. 每次 `Arc::clone/drop` 都有原子计数成本，但通常远小于网络或模型调用；先保证所有权正确。
2. `Arc` 不是垃圾回收器，不会检测环。
3. `Weak::upgrade().is_some()` 之后不能丢掉结果再重新假定对象还活着，应使用取得的强引用。
4. 不要为了满足 `'static` 把所有参数都包进 `Arc`；能 move 独占值就保持独占。
5. 不要用 `strong_count == 1` 作为并发协议，它只是观察瞬间。

## 项目映射

- [`ReactAgentBuilder`](../../../src/agent/react/builder.rs)：共享 LLM、回调和 Store。
- [`workflow/node.rs`](../../../echo-orchestration/src/workflow/node.rs)：共享动态 Agent。
- [`ToolManager`](../../../echo-execution/src/tools.rs)：共享工具管理器。
- [`arc_weak.rs`](../../src/smart_pointers/arc_weak.rs)：最小强弱引用注册表。

## 练习

1. 运行示例，分别在 clone、drop 前后打印 `counts()`，解释每次变化。
2. 给 `AgentRegistry` 增加 `active_names()`，忽略 `upgrade()` 失败的条目。
3. 画出父任务、PlanTask、SubagentRun 的强/弱引用方向，并标明真正所有者。
4. 将一个 `Rc<String>` move 进 `tokio::spawn`，只阅读编译错误；再说明为何 `Arc<String>` 合法。

下一章：[内部可变性与同步](10-interior-mutability.md)。
