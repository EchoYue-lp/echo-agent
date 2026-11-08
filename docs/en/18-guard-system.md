# Guard System — Content Filtering

## What It Is

The Guard system filters content at four production boundaries: user input,
effective tool arguments, tool results, and final text answers. A guard can
pass, warn, block, or transform content where transformation is permitted.

| Direction | Production boundary | Transform |
| --- | --- | --- |
| `Input` | User text before the model context | Allowed |
| `ToolInput` | Final effective JSON arguments after rewrites, before invocation | Rejected (block-only) |
| `ToolOutput` | Tool result before output budgeting and terminal observation | Allowed |
| `Output` | Model text final answer before callbacks and delivery | Allowed |

原生最终候选按 `Output` 检查，工具结果按 `ToolOutput` 检查。新执行不使用控制工具或 Token/Think 边界推断文字类型；整段校验不提前公开候选，协议原件与批准呈现投影分开保留。详见[原生消息合同](../zh/43-native-message-execution.md)。

---

## Problem It Solves

Without guards, an Agent might:
- **Leak sensitive data**: Output PII, credentials, or internal documents
- **Generate harmful content**: Hate speech, violence, illegal instructions
- **Violate policies**: Exceed rate limits, access forbidden resources
- **Allow prompt injection**: Malicious user input manipulating behavior

Guards act as security checkpoints in the Agent pipeline.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Guard Pipeline                                   │
│                                                                      │
│   User Input                                                        │
│       │                                                              │
│       ▼                                                              │
│   ┌─────────────────────────────────────────┐                      │
│   │           Input Guards                   │                      │
│   │  ┌─────────┐ ┌─────────┐ ┌─────────┐   │                      │
│   │  │ PII     │ │ Injection│ │ Policy  │   │                      │
│   │  │ Filter  │ │ Detector │ │ Checker │   │                      │
│   │  └─────────┘ └─────────┘ └─────────┘   │                      │
│   └─────────────────────────────────────────┘                      │
│       │                                                              │
│       │ Pass → LLM Processing                                       │
│       │ Block → Return error                                        │
│       ▼                                                              │
│   ┌─────────────────────────────────────────┐                      │
│   │           Output Guards                  │                      │
│   │  ┌─────────┐ ┌─────────┐ ┌─────────┐   │                      │
│   │  │ Length  │ │ LLM     │ │ Secret  │   │                      │
│   │  │ Limiter │ │ Filter  │ │ Redactor│   │                      │
│   │  └─────────┘ └─────────┘ └─────────┘   │                      │
│   └─────────────────────────────────────────┘                      │
│       │                                                              │
│       ▼                                                              │
│   Final Output                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

ContentGuard reports match positions as UTF-8 byte offsets into the original
text. Redaction merges overlapping match ranges before rendering, choosing the
widest match's marker (existing detection order breaks equal-width ties). A phone
inside a card is therefore covered once, while adjacent distinct matches keep
their own markers. Rendering iterates Unicode characters and preserves surrounding
Chinese text and emoji without reusing offsets against already-modified text.

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
    Input,      // User -> Agent
    Output,     // Model text final answer -> User
    ToolInput,  // Effective tool arguments -> Tool
    ToolOutput, // Tool result -> Agent
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

Rule-based guards use patterns for instant filtering:

```rust
use echo_agent::guard::rule::{RuleGuard, RuleGuardBuilder};

let guard = RuleGuardBuilder::new("no-pii")
    .blocked_pattern(r"\b\d{3}-\d{2}-\d{4}\b")
    .blocked_keyword("password")
    .direction(GuardDirection::Output)
    .build();

// Test
let result = guard.check("My SSN is 123-45-6789", GuardDirection::Output).await?;
assert!(matches!(result, GuardResult::Block { .. }));
```

---

## LlmGuard

LLM-based guards provide semantic understanding:

```rust
use echo_agent::guard::llm::LlmGuard;

let guard = LlmGuard::with_prompt(
    "review",
    review_llm_client,
    "Review the content and return JSON: {\"safe\": true} or {\"safe\": false, \"reason\": \"...\"}",
).with_directions(vec![GuardDirection::Output]);

// The LLM evaluates content semantically
let result = guard.check("...", GuardDirection::Output).await?;
```

---

## GuardManager

```rust
use echo_agent::guard::{GuardManager, GuardDirection};

let mut manager = GuardManager::new();

// Guards run in registration order and select their own directions.
manager.add(Arc::new(injection_guard));
manager.add(Arc::new(policy_guard));
manager.add(Arc::new(pii_guard));
manager.add(Arc::new(llm_guard));

// Check input
match manager.check_all("User's query", GuardDirection::Input).await? {
    GuardResult::Pass => { /* proceed */ }
    GuardResult::Block { reason } => { 
        return Err(Error::Blocked(reason));
    }
    GuardResult::Transform { content, .. } => {
        // Use modified content
    }
    GuardResult::Warn { .. } => { /* proceed with warning */ }
}

// Check output
match manager.check_all("Agent's response", GuardDirection::Output).await? {
    GuardResult::Pass => { /* return to user */ }
    GuardResult::Block { reason } => { /* redact or error */ }
    GuardResult::Transform { content, .. } => { /* return modified */ }
    GuardResult::Warn { .. } => { /* return original */ }
}
```

---

## Integration with Agent

```rust
use echo_agent::prelude::*;

let mut agent = ReactAgentBuilder::new()
    .model("qwen3-max")
    .system_prompt("You are a helpful assistant")
    .build()?;

// Create and attach guard manager
let mut guard_manager = GuardManager::new();
guard_manager.add(Arc::new(injection_guard));
guard_manager.add(Arc::new(pii_guard));

agent.set_guard_manager(guard_manager);

// Guards are automatically checked during execute()
let result = agent.execute("What is my SSN?").await?;
// Output guard will block if PII is in response
```

---

## Custom Guard with Macro

```rust
use echo_agent::{guard, prelude::*};

#[guard(name = "length-limit")]
async fn check_length(content: &str, direction: GuardDirection) -> Result<GuardResult> {
    if content.chars().count() > 10000 {
        Ok(GuardResult::Block {
            reason: format!("Content too long: {} chars", content.chars().count())
        })
    } else {
        Ok(GuardResult::Pass)
    }
}

// Use the generated LengthLimitGuard
manager.add(Arc::new(LengthLimitGuard));
```

---

## Guard Chaining

Multiple guards are evaluated in sequence:

```rust
manager.add(Arc::new(guard1));  // First
manager.add(Arc::new(guard2));  // Second
manager.add(Arc::new(guard3));  // Third

// Execution:
// 1. guard1.check() → if Block, stop and return
// 2. guard2.check() → if Block, stop and return
// 3. guard3.check() → if Block, stop and return
// 4. All passed → proceed
```

First guard to return `Block` stops the chain. A guard error propagates to the
caller for fail-closed handling; it never becomes `Warn`.

---

## Built-in Guards

| Guard | Type | Purpose |
|-------|------|---------|
| `RuleGuard` | Rule | Pattern-based blocking |
| `LlmGuard` | LLM | Semantic content analysis |
| `RuleGuard::max_length` | Rule | Block oversized content |
| `ContentGuard` | Content | Detect, reject, or redact sensitive content |

---

## Best Practices

1. **Layer your guards**: Fast rule-based first, expensive LLM-based last
2. **Be specific with patterns**: Avoid overly broad regex
3. **Log blocked content**: For auditing and tuning
4. **Test thoroughly**: Ensure legitimate content isn't blocked
5. **Consider Transform vs Block**: Redact content only at boundaries that permit transformation

---

## Performance Considerations

| Guard Type | Latency | Cost |
|------------|---------|------|
| RuleGuard | < 1ms | Free |
| LlmGuard | 100-500ms | API call |

Place LlmGuard at the end of the chain to avoid unnecessary calls.

See: `echo-agent-learning/examples/demo19_guard.rs`
