# 02 变量、类型、表达式与控制流

这一章从 Rust 程序最小的组成部分开始。目标不是背语法，而是能逐行读懂
[`chapter_01_basics.rs`](../../examples/chapter_01_basics.rs)，并能解释每个值的
类型、作用域和控制流。

## 先运行，再阅读

```bash
cargo run -p echo_rust_learning --example chapter_01_basics
```

你会看到分数摘要、安全的数组访问、坐标交换、倒计时和重试决策。示例没有读取网络或环境变量，
所以输出只由代码决定。

## 变量绑定与 `mut`

Rust 用 `let` 创建绑定：

```rust
let language = "Rust";
let mut attempts = 0_u32;
attempts = attempts.saturating_add(1);
```

绑定默认不可变。不可变不是“值永远不能变化”，而是不能通过这个绑定重新赋值。需要重新赋值时
显式写 `mut`，让状态变化在代码审查时一眼可见。

下面不能编译：

```rust,compile_fail
let attempts = 0;
attempts = 1;
```

编译器会提示不能给不可变变量赋值。修复方式不是习惯性地给所有变量加 `mut`，而是先判断这个
算法是否确实需要可变状态。

## Shadowing 不是可变赋值

同名 `let` 会创建一个新绑定，并遮蔽旧绑定：

```rust
let input = " 8 ";
let input = input.trim();
let input: usize = input.parse().unwrap_or(0);
```

三次 `input` 的类型分别可以不同。shadowing 适合表达“同一概念经过转换”，`mut` 适合表达“同一
变量随算法推进变化”。新手常把两者混在一起；判断关键是类型是否需要改变、旧值是否还应可见。

## 标量类型

标量一次保存一个值。

### 整数

| 类型 | 示例 | 常见用途 |
|------|------|----------|
| `u8` | `100_u8` | 百分比、字节、很小的无符号范围 |
| `u32` | `8_u32` | 重试次数、协议中的非负整数 |
| `usize` | `items.len()` | 集合长度和索引，宽度随平台变化 |
| `i32` | `-1_i32` | 可为负的普通整数 |
| `u64` | 时间戳、累计量 | 更大范围的非负值 |

Rust 不会隐式把 `u32` 转成 `usize`。转换必须显式，因为目标平台上范围可能不同：

```rust
let count = usize::try_from(value).map_err(|_| "count does not fit usize")?;
```

对可能溢出的运算选择明确策略：

- `checked_add`：溢出返回 `None`，适合错误必须上报的业务量。
- `saturating_add`：溢出停在最大值，适合展示计数或上限。
- `wrapping_add`：按补码回绕，只适合协议、哈希等明确需要回绕的算法。

### 浮点数

`f32` 和 `f64` 表示近似实数。`f64` 是常见默认值。不要用 `==` 比较经过复杂计算的浮点结果；
通常比较差值是否小于容差。金额、精确计数不要随意使用浮点。

### 布尔与字符

`bool` 只有 `true`/`false`。`char` 表示一个 Unicode 标量值，使用单引号：

```rust
let enabled = true;
let crab = '🦀';
```

一个 `char` 不等于用户感知的“一个字形”：某些组合字符由多个 Unicode 标量组成。本项目规定的
`chars()` 截断保证 UTF-8 不被切坏，但不是完整的 grapheme cluster 分割。

## 复合类型：tuple、array 与 slice

### tuple

tuple 长度固定，每个位置可以是不同类型：

```rust
let point: (i32, i32) = (10, 20);
let (x, y) = point;
```

字段开始具有业务含义时，优先定义 struct；`(String, usize, bool)` 很快会让调用者忘记每个位置的
含义。

### array

array 长度是类型的一部分，数据连续存放：

```rust
let scores: [u32; 3] = [72, 88, 95];
```

直接 `scores[index]` 在越界时会 panic。本项目对不可信索引使用：

```rust
let value: Option<&u32> = scores.get(index);
```

### slice

slice `&[T]` 是对一段连续元素的借用，不拥有数据，也不把长度写进静态类型。函数接收 slice 后既
能处理 array，也能处理 Vec：

```rust
pub fn safe_item<T>(items: &[T], index: usize) -> Option<&T> {
    items.get(index)
}
```

完整实现见 [`fundamentals.rs`](../../src/fundamentals.rs)。

## 类型推断与类型标注

编译器通常能从上下文推断类型：

```rust
let retries = 3;              // 通常推断为 i32
let names = Vec::<String>::new();
let limit: usize = "8".parse().map_err(|_| "invalid limit")?;
```

空集合、`parse()` 和泛型 `collect()` 常需要额外类型信息。可以标在变量、泛型参数或 collect 上：

```rust
let values = iterator.collect::<Vec<_>>();
```

`_` 表示让编译器推断这一部分，而不是“任意类型”。

## 函数、语句与表达式

Rust 函数参数和返回类型都要声明：

```rust
fn double(value: u32) -> u32 {
    value.saturating_mul(2)
}
```

最后一行没有分号，因此它是函数返回表达式。加上分号会把表达式变成语句，结果变为单元类型
`()`. 这是新手常见错误：

```rust,compile_fail
fn double(value: u32) -> u32 {
    value * 2;
}
```

代码块、`if`、`match` 和 `loop` 都可以产生值：

```rust
let label = if ready { "ready" } else { "waiting" };
```

`if` 两个分支必须产生兼容类型。Rust 不做 JavaScript 风格的隐式类型拼接。

## 控制流

### if/else

条件必须是 `bool`，整数不会自动当成真假：

```rust,compile_fail
let count = 1;
if count { /* ... */ }
```

### loop、while 与 for

- `loop`：无限循环，通常由 `break`、`return` 或取消信号结束。
- `while condition`：条件为真时继续。
- `for item in iterator`：遍历序列，最不容易出现边界错误。

```rust
for score in &scores {
    println!("{score}");
}

let value = loop {
    if ready() {
        break 42;
    }
};
```

`break value` 让 loop 成为表达式。嵌套循环可以加标签，但复杂嵌套通常提示函数应拆分。

## `match` 与穷尽性

`match` 根据值的结构选择分支，编译器要求覆盖全部可能情况：

```rust
match decide_attempt(1, 3) {
    AttemptDecision::Start => println!("start"),
    AttemptDecision::Retry { remaining } => println!("{remaining}"),
    AttemptDecision::Stop => println!("stop"),
}
```

新增 enum 变体后，所有未覆盖的 match 会编译失败。这是类型系统帮助维护状态机，而不是额外负担。
第 3 章会深入模式解构。

## 注释与文档注释

- `//`：解释局部实现中“为什么这样做”。
- `///`：为紧随其后的公开项生成 rustdoc。
- `//!`：为当前 module 或 crate 编写文档。

不要用注释重复代码表面动作。优先解释不变量、边界选择和外部协议约束。

## 常见编译错误

| 现象 | 常见原因 | 处理方式 |
|------|----------|----------|
| `mismatched types` | 分支或参数类型不同 | 从 expected/found 找到真正边界，不盲目 `as` |
| `cannot assign twice` | 绑定没有 `mut` | 判断是否需要可变状态或 shadowing |
| `type annotations needed` | 泛型结果缺上下文 | 在变量、泛型或 `collect` 标类型 |
| `index out of bounds` | 直接索引不可信位置 | 使用 `get` 并处理 Option |
| arithmetic overflow | 整数超出范围 | 选择 checked/saturating/wrapping 语义 |

## 项目映射

- [`fundamentals.rs`](../../src/fundamentals.rs)：本章完整可运行实现。
- [`echo-core/src/tools/mod.rs`](../../../echo-core/src/tools/mod.rs)：大量 enum、slice 和数值配置。
- [`src/agent/config.rs`](../../../src/agent/config.rs)：结构体字段、默认值和 Builder 输入。

## 练习

1. 给 `summarize_scores` 增加 `count` 字段，并处理从 usize 到输出类型的转换。
2. 用 `loop { break value }` 实现一个最多尝试三次的纯函数。
3. 写一个接收 `&[i32]` 的函数，使用 `get` 返回第一个负数的位置。
4. 把 chapter 01 中一个 `println!` 改成错误类型，阅读 expected/found 后修复。

下一章：[struct、enum 与模式匹配](03-domain-modeling-pattern-matching.md)。
