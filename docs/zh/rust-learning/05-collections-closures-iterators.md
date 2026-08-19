# 05 集合、闭包与迭代器

Agent 系统离不开消息列表、工具注册表、可见工具集合和任务索引。本章不仅介绍集合 API，还解释
迭代方式如何改变所有权。

## 运行本章示例

```bash
cargo run -p echo_rust_learning --example chapter_05_collections_iterators
```

完整实现见 [`collections.rs`](../../../echo-rust-learning/src/collections.rs)。

## `Vec<T>`：有序、可增长的连续序列

```rust
let mut tasks = Vec::new();
tasks.push(task);
let first = tasks.first();
let maybe = tasks.get(index);
```

Vec 拥有其中的元素。容量不足时会重新分配并移动元素，所以不要长期保存指向 Vec 内部元素的引用
后再修改 Vec。常用方法：

- `len`/`is_empty`：长度检查。
- `push`/`pop`：尾部增删。
- `get`/`get_mut`：安全索引。
- `retain`：保留满足条件的元素。
- `drain`：移出一个范围。
- `sort_by`/`sort_by_key`：原地排序。

已知大概数量时使用 `Vec::with_capacity` 可减少重新分配，但不要为了猜容量制造复杂配置。

## `HashMap<K, V>`：按键查找

```rust
let mut positions = HashMap::new();
positions.insert(title.clone(), index);
let task = positions.get(title).and_then(|index| tasks.get(*index));
```

HashMap 不保证迭代顺序。需要稳定输出时对键排序，或同时维护有序 Vec。教学 `TaskCatalog` 用 Vec
保持插入顺序、HashMap 提供快速标题查找，这种双结构必须由同一 API 维护不变量。

### Entry API

词频统计只查找一次：

```rust
let count = frequencies.entry(word).or_insert(0usize);
*count = count.saturating_add(1);
```

`or_insert` 返回可变引用。引用仍存活时不能再次任意借用整个 map，这是借用规则在防止迭代器或
引用失效。

## `HashSet<T>`：唯一成员集合

HashSet 只关心某个值是否存在：

```rust
let inserted = tags.insert("rust".to_string());
```

第一次插入返回 true，重复插入返回 false。适合可见工具名、权限集合、去重标识。和 HashMap 一样，
输出顺序不稳定；测试中不要依赖裸 HashSet 的迭代顺序。

## 三种迭代方式与所有权

假设 `values: Vec<String>`：

| 写法 | item 类型 | 之后能否使用 Vec | 用途 |
|------|-----------|------------------|------|
| `values.iter()` | `&String` | 能 | 只读遍历 |
| `values.iter_mut()` | `&mut String` | 能 | 原地修改 |
| `values.into_iter()` | `String` | 不能 | 消耗集合、转移元素 |

`for value in &values` 等价于只读迭代，`for value in &mut values` 等价于可变迭代。看到 move 错误时，
先确认你是否真的需要取得元素所有权。

## Iterator 是惰性的

调用 `map`、`filter` 只构造适配器；直到 `collect`、`sum`、`for_each`、`next` 等消费者出现才执行：

```rust
let normalized = values
    .into_iter()
    .map(|value| value.trim().to_lowercase())
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>();
```

常用适配器：

- `map`：一对一转换。
- `filter`：保留满足条件的项，闭包收到借用。
- `filter_map`：同时过滤和转换 Option。
- `flat_map`：每项产生多个项并展平。
- `enumerate`：附加从 0 开始的位置。
- `zip`：配对两个序列，任一结束即结束。
- `take`/`skip`：限制或跳过数量。
- `find`/`position`：短路查找。
- `try_fold`：累计过程中允许返回错误。

Iterator 通常能表达边界和所有权，但不是越短越好。复杂多阶段业务逻辑拆成具名函数，通常比一条
二十层链更可维护。

## 闭包与捕获

闭包语法是 `|参数| 表达式`：

```rust
let keyword = String::from("Rust");
let titles = catalog.matching_titles(|task| task.title.contains(&keyword));
```

编译器根据用法决定捕获方式：

- 只读取：借用，闭包通常实现 `Fn`。
- 修改捕获值：可变借用，通常实现 `FnMut`。
- 移走捕获值：实现 `FnOnce`。

`move` 闭包强制取得捕获值所有权，常用于 `tokio::spawn`。`move` 不等于深拷贝；捕获 Arc 时只是
移动某个 Arc 所有者，捕获 String 时移动整个 String。

## 闭包 trait 的关系

- `Fn`：可以多次通过共享引用调用。
- `FnMut`：调用可能修改环境，需要可变访问。
- `FnOnce`：至少能调用一次；所有闭包都实现 FnOnce。

API 应声明满足需求的最弱约束。只调用一次的回调不必强迫实现 Fn。

## `collect` 的目标类型

Iterator 不知道你想收集成 Vec、HashMap 还是 Result。目标类型可来自变量标注或 turbofish：

```rust
let names: Vec<String> = iterator.collect();
let names = iterator.collect::<Vec<_>>();
```

一个重要模式是收集 Result：

```rust
let values = inputs
    .iter()
    .map(|input| input.parse::<u32>())
    .collect::<Result<Vec<_>, _>>()?;
```

遇到第一个错误就停止，并保留错误，不需要手写临时 Vec。

## 字符串集合的分配成本

`HashMap<String, V>::get` 可以用 `&str` 查找，通常不必为每次查询 `to_string()`。插入时 Map 必须
拥有键，才需要 String。区分查询借用和持久化所有权能减少无意义 clone。

## 常见错误

| 错误 | 原因 | 修复思路 |
|------|------|----------|
| use of moved value: values | `into_iter` 消耗了 Vec | 若还需 Vec，改用 iter；否则接受 move |
| cannot borrow map as mutable more than once | Entry 返回的引用仍存活 | 缩小引用作用域，再做下一次操作 |
| closure may outlive current function | 后台任务借用了局部值 | 明确 clone/Arc 后使用 `move` |
| type annotations needed for collect | 目标集合不明确 | 标注变量或 `collect::<Vec<_>>()` |

## 项目映射

- [`ToolVisibilityState`](../../../echo-core/src/tools/mod.rs)：HashSet 和 RwLock 管理工具可见性。
- [`ToolManager`](../../../echo-execution/src/tools.rs)：工具名到实现的注册表。
- [`InMemoryStore`](../../../echo-state/src/memory/store.rs)：嵌套 HashMap 与异步锁。
- [`ContextSelector`](../../../src/context/selector.rs)：迭代、评分、排序和筛选。

## 练习

1. 给 TaskCatalog 增加 remove，确保 Vec 与 positions 不会失去一致性。
2. 使用 `collect::<Result<Vec<_>, _>>()` 解析一组数字文本。
3. 分别写 Fn、FnMut、FnOnce 闭包，观察编译器允许的调用次数。
4. 给词频统计加入 Unicode 文本测试，并说明 `split_whitespace` 的边界。

下一章：[Option、Result 与错误处理](06-option-result-errors.md)。
