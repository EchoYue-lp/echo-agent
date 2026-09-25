# Guard 系统 —— 内容过滤

## 是什么

Guard 系统在用户输入、有效工具参数、工具结果和最终文本答案四个生产边界过滤内容。
护栏可放行、警告、阻断，或在允许的边界转换内容。

| 方向 | 生产边界 | 转换 |
| --- | --- | --- |
| `Input` | 用户文本进入模型上下文前 | 允许 |
| `ToolInput` | 全部重写后的最终 JSON 参数、工具调用前 | 拒绝（仅阻断） |
| `ToolOutput` | 工具结果进入预算与终态观察前 | 允许 |
| `Output` | 模型文本最终答案进入回调和交付前 | 允许 |

`final_answer` 工具结果只按 `ToolOutput` 检查，不在写入 transcript 后再次按
`Output` 检查。护栏后端错误 fail-closed，不降级为警告。ToolInput 转换会被拒绝，
因为审批收据绑定的是未经再次修改的有效参数。
文本答案的 `Token` 与 `FinalAnswer` 事件携带相同的受检内容；provider 失败时
的部分内容也要经过检查才会以 `Token` 交付。
配置 GuardManager 后，工具流的 chunk 和进度被抑制，caller 只收到经过护栏和
输出预算的权威终态 `ToolResult`；未配置时保留原有
stdout/stderr 实时流。工具失败且无输出时，受检诊断统一进入返回错误、Trace、
Audit、callback 和 transcript。
结构化 data、非空 metadata、携带内容的结果 kind 和 MIME 类型会分别规范序列化为
文本检查；`Pass` 保留已检查的结构，任一字段被阻断或转换时，无法从受检文本无损
重建的平行展示字段会撤销。配置 Guard 时，即使文本 `Pass`，图片 URL/模型富内容
和护栏前 artifact 引用也会抑制，因为文本检查无法证明像素或未见字节安全。
typed failure 与已确认 effect 事实保留。用户安装的 PostToolUse hook 仍按既有顺序
在展示护栏前运行，可看到原始结果；它不是受检消费者边界。
`ToolFailure.postcondition` 和 `idempotency_key` 的自由文本也会分别检查。
idempotency key 被修改时直接撤销，不伪造新的重试身份。post-use hook 若将原始
工具输出写入阻断原因，caller 错误与 skill telemetry 改用受检的 `ToolResult.error`。
已确认的 typed effect path 仍依 ADR 0074 在 caller 与 Trace 可见；Guard 不承诺
通用脱敏这些恢复事实。

---

## 解决什么问题

没有护栏，Agent 可能：
- **泄露敏感数据**：输出 PII、凭证或内部文档
- **生成有害内容**：仇恨言论、暴力、非法指导
- **违反策略**：超出速率限制、访问禁止资源
- **允许提示词注入**：恶意用户输入操纵行为

护栏作为 Agent 管道中的安全检查点。

---

## 架构

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Guard 管道                                       │
│                                                                      │
│   用户输入                                                          │
│       │                                                              │
│       ▼                                                              │
│   ┌─────────────────────────────────────────┐                      │
│   │           输入护栏                       │                      │
│   │  ┌─────────┐ ┌─────────┐ ┌─────────┐   │                      │
│   │  │ PII     │ │ 注入    │ │ 策略    │   │                      │
│   │  │ 过滤器  │ │ 检测器  │ │ 检查器  │   │                      │
│   │  └─────────┘ └─────────┘ └─────────┘   │                      │
│   └─────────────────────────────────────────┘                      │
│       │                                                              │
│       │ 通过 → LLM 处理                                             │
│       │ 阻止 → 返回错误                                             │
│       ▼                                                              │
│   ┌─────────────────────────────────────────┐                      │
│   │           输出护栏                       │                      │
│   │  ┌─────────┐ ┌─────────┐ ┌─────────┐   │                      │
│   │  │ 长度    │ │ LLM     │ │ 敏感词  │   │                      │
│   │  │ 限制器  │ │ 过滤器  │ │ 脱敏器  │   │                      │
│   │  └─────────┘ └─────────┘ └─────────┘   │                      │
│   └─────────────────────────────────────────┘                      │
│       │                                                              │
│       ▼                                                              │
│   最终输出                                                          │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Guard Trait

```rust
pub trait Guard: Send + Sync {
    fn name(&self) -> &str;
    
    fn check<'a>(
        &'a self,
        content: &'a str,
        direction: GuardDirection,
    ) -> BoxFuture<'a, Result<GuardResult>>;
}

pub enum GuardDirection {
    Input,      // 用户 -> Agent
    Output,     // 模型文本最终答案 -> 用户
    ToolInput,  // 有效工具参数 -> Tool
    ToolOutput, // 工具结果 -> Agent
}

pub enum GuardResult {
    Pass,
    Block { reason: String },
    Warn { reasons: Vec<String> },
    Transform { content: String, reasons: Vec<String> },
}
```

---

## RuleGuard

基于规则的护栏使用模式进行即时过滤：

```rust
use echo_agent::guard::rule::{RuleGuard, RuleGuardBuilder};

let guard = RuleGuardBuilder::new("no-pii")
    .blocked_pattern(r"\b\d{3}-\d{2}-\d{4}\b")
    .blocked_keyword("password")
    .direction(GuardDirection::Output)
    .build();

// 测试
let result = guard.check("我的 SSN 是 123-45-6789", GuardDirection::Output).await?;
assert!(matches!(result, GuardResult::Block { .. }));
```

---

## LlmGuard

基于 LLM 的护栏提供语义理解：

```rust
use echo_agent::guard::llm::LlmGuard;

let guard = LlmGuard::with_prompt(
    "review",
    review_llm_client,
    "检查内容并返回 JSON：{\"safe\": true} 或 {\"safe\": false, \"reason\": \"...\"}",
).with_directions(vec![GuardDirection::Output]);

// LLM 语义评估内容
let result = guard.check("...", GuardDirection::Output).await?;
```

---

## GuardManager

```rust
use echo_agent::guard::{GuardManager, GuardDirection};

let mut manager = GuardManager::new();

// 按注册顺序运行，各护栏自行选择方向。
manager.add(Arc::new(injection_guard));
manager.add(Arc::new(policy_guard));
manager.add(Arc::new(pii_guard));
manager.add(Arc::new(llm_guard));

// 检查输入
match manager.check_all("用户的查询", GuardDirection::Input).await? {
    GuardResult::Pass => { /* 继续 */ }
    GuardResult::Block { reason } => { 
        return Err(Error::Blocked(reason));
    }
    GuardResult::Transform { content, .. } => {
        // 使用修改后的内容
    }
    GuardResult::Warn { .. } => { /* 带警告继续 */ }
}

// 检查输出
match manager.check_all("Agent 的响应", GuardDirection::Output).await? {
    GuardResult::Pass => { /* 返回给用户 */ }
    GuardResult::Block { reason } => { /* 脱敏或错误 */ }
    GuardResult::Transform { content, .. } => { /* 返回修改后的 */ }
    GuardResult::Warn { .. } => { /* 返回原始内容 */ }
}
```

---

## 与 Agent 集成

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("你是一个有帮助的助手")
    .build()?;

// 创建并附加护栏管理器
let mut guard_manager = GuardManager::new();
guard_manager.add(Arc::new(injection_guard));
guard_manager.add(Arc::new(pii_guard));

agent.set_guard_manager(guard_manager);

// execute() 期间自动检查护栏
let result = agent.execute("我的 SSN 是什么？").await?;
// 输出护栏会阻止包含 PII 的响应
```

---

## 使用宏定义自定义护栏

```rust
use echo_agent::{guard, prelude::*};

#[guard(name = "length-limit")]
async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
    if content.chars().count() > 10000 {
        Ok(GuardResult::Block {
            reason: format!("内容过长: {} 字符", content.chars().count())
        })
    } else {
        Ok(GuardResult::Pass)
    }
}

// 使用生成的 LengthLimitGuard
manager.add(Arc::new(LengthLimitGuard));
```

---

## 护栏链

多个护栏按顺序评估：

```rust
manager.add(Arc::new(guard1));  // 第一个
manager.add(Arc::new(guard2));  // 第二个
manager.add(Arc::new(guard3));  // 第三个

// 执行顺序：
// 1. guard1.check() → 若 Block，停止并返回
// 2. guard2.check() → 若 Block，停止并返回
// 3. guard3.check() → 若 Block，停止并返回
// 4. 全部通过 → 继续
```

第一个返回 `Block` 的护栏停止链条。护栏错误传播给调用方执行 fail-closed，
不会变成 `Warn`。

---

## 内置护栏

| 护栏 | 类型 | 用途 |
|------|------|------|
| `RuleGuard` | 规则 | 基于模式的阻止 |
| `LlmGuard` | LLM | 语义内容分析 |
| `RuleGuard::max_length` | 规则 | 阻止过长内容 |
| `ContentGuard` | 内容 | 检测、拒绝或脱敏敏感内容 |

---

## 最佳实践

1. **分层护栏**：快速的规则型在前，昂贵的 LLM 型在后
2. **模式要具体**：避免过于宽泛的正则
3. **记录被阻止的内容**：用于审计和调优
4. **充分测试**：确保合法内容不被阻止
5. **考虑 Transform 与 Block**：仅在允许转换的边界执行脱敏

---

## 性能考量

| 护栏类型 | 延迟 | 成本 |
|----------|------|------|
| RuleGuard | < 1ms | 免费 |
| LlmGuard | 100-500ms | API 调用 |

将 LlmGuard 放在链条末尾以避免不必要的调用。

对应示例：`echo-agent-learning/examples/demo19_guard.rs`
