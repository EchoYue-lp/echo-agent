# Guard 系统 —— 内容过滤

## 是什么

Guard 系统提供输入/输出内容过滤，强制执行安全、合规和策略规则。护栏可以在内容到达 LLM 之前（输入护栏）或返回给用户之前（输出护栏）阻止或修改内容。

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
    Input,   // 用户 → Agent
    Output,  // Agent → 用户
}

pub enum GuardResult {
    Pass,
    Block { reason: String },
    Modify { content: String },
}
```

---

## RuleGuard

基于规则的护栏使用模式进行即时过滤：

```rust
use echo_agent::guard::rule::{RuleGuard, RuleGuardBuilder};

let guard = RuleGuardBuilder::new("no-pii")
    // 阻止正则模式
    .block_regex(r"\b\d{3}-\d{2}-\d{4}\b")      // SSN
    .block_regex(r"\b[A-Z]{2}\d{6}\b")          // 护照号
    .block_regex(r"\b[\w.-]+@[\w.-]+\.\w+\b")   // 邮箱
    // 允许模式（白名单）
    .allow_regex(r"\b\d{4}\b")                  // 允许 4 位数字
    // 自定义规则
    .block_if(|content| content.contains("password"))
    .build()?;

// 测试
let result = guard.check("我的 SSN 是 123-45-6789", GuardDirection::Output).await?;
assert!(matches!(result, GuardResult::Block { .. }));
```

---

## LlmGuard

基于 LLM 的护栏提供语义理解：

```rust
use echo_agent::guard::llm::LlmGuard;

let guard = LlmGuard::new("qwen3-max")
    .with_prompt(|content, direction| format!(
        "分析以下内容是否存在问题：\n\n{}\n\n\
         检查：有害内容、PII、敏感信息。\
         返回 'PASS' 或 'BLOCK: 原因'",
        content
    ))
    .with_max_tokens(100);

// LLM 语义评估内容
let result = guard.check("...", GuardDirection::Output).await?;
```

---

## GuardManager

```rust
use echo_agent::guard::{GuardManager, GuardDirection};

let mut manager = GuardManager::new();

// 添加输入护栏
manager.add_input_guard(Box::new(injection_guard));
manager.add_input_guard(Box::new(policy_guard));

// 添加输出护栏
manager.add_output_guard(Box::new(pii_guard));
manager.add_output_guard(Box::new(llm_guard));

// 检查输入
match manager.check_input("用户的查询").await? {
    GuardResult::Pass => { /* 继续 */ }
    GuardResult::Block { reason } => { 
        return Err(Error::Blocked(reason));
    }
    GuardResult::Modify { content } => {
        // 使用修改后的内容
    }
}

// 检查输出
match manager.check_output("Agent 的响应").await? {
    GuardResult::Pass => { /* 返回给用户 */ }
    GuardResult::Block { reason } => { /* 脱敏或错误 */ }
    GuardResult::Modify { content } => { /* 返回修改后的 */ }
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
let guard_manager = GuardManager::new()
    .add_input_guard(Box::new(injection_guard))
    .add_output_guard(Box::new(pii_guard));

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
    if content.len() > 10000 {
        Ok(GuardResult::Block { 
            reason: format!("内容过长: {} 字符", content.len()) 
        })
    } else {
        Ok(GuardResult::Pass)
    }
}

// 使用生成的 LengthLimitGuard
manager.add_output_guard(Box::new(LengthLimitGuard));
```

---

## 护栏链

多个护栏按顺序评估：

```rust
manager.add_input_guard(Box::new(guard1));  // 第一个
manager.add_input_guard(Box::new(guard2));  // 第二个
manager.add_input_guard(Box::new(guard3));  // 第三个

// 执行顺序：
// 1. guard1.check() → 若 Block，停止并返回
// 2. guard2.check() → 若 Block，停止并返回
// 3. guard3.check() → 若 Block，停止并返回
// 4. 全部通过 → 继续
```

第一个返回 `Block` 的护栏停止链条。

---

## 内置护栏

| 护栏 | 类型 | 用途 |
|------|------|------|
| `RuleGuard` | 规则 | 基于模式的阻止 |
| `LlmGuard` | LLM | 语义内容分析 |
| `LengthGuard` | 规则 | 阻止过长内容 |
| `SecretRedactor` | 规则 | 用 *** 脱敏敏感词 |

---

## 最佳实践

1. **分层护栏**：快速的规则型在前，昂贵的 LLM 型在后
2. **模式要具体**：避免过于宽泛的正则
3. **记录被阻止的内容**：用于审计和调优
4. **充分测试**：确保合法内容不被阻止
5. **考虑 Modify vs Block**：有时脱敏比阻止更好

---

## 性能考量

| 护栏类型 | 延迟 | 成本 |
|----------|------|------|
| RuleGuard | < 1ms | 免费 |
| LlmGuard | 100-500ms | API 调用 |

将 LlmGuard 放在链条末尾以避免不必要的调用。

对应示例：`echo-agent-learning/examples/demo19_guard.rs`