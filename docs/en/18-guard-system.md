# Guard System — Content Filtering

## What It Is

The Guard system provides input/output content filtering to enforce safety, compliance, and policy rules. Guards can block or modify content before it reaches the LLM (input guards) or before it's returned to the user (output guards).

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
    Input,   // User → Agent
    Output,  // Agent → User
}

pub enum GuardResult {
    Pass,
    Block { reason: String },
    Modify { content: String },
}
```

---

## RuleGuard

Rule-based guards use patterns for instant filtering:

```rust
use echo_agent::guard::rule::{RuleGuard, RuleGuardBuilder};

let guard = RuleGuardBuilder::new("no-pii")
    // Block regex patterns
    .block_regex(r"\b\d{3}-\d{2}-\d{4}\b")      // SSN
    .block_regex(r"\b[A-Z]{2}\d{6}\b")          // Passport
    .block_regex(r"\b[\w.-]+@[\w.-]+\.\w+\b")   // Email
    // Allow patterns (whitelist)
    .allow_regex(r"\b\d{4}\b")                  // Allow 4-digit numbers
    // Custom rules
    .block_if(|content| content.contains("password"))
    .build()?;

// Test
let result = guard.check("My SSN is 123-45-6789", GuardDirection::Output).await?;
assert!(matches!(result, GuardResult::Block { .. }));
```

---

## LlmGuard

LLM-based guards provide semantic understanding:

```rust
use echo_agent::guard::llm::LlmGuard;

let guard = LlmGuard::new("qwen3-max")
    .with_prompt(|content, direction| format!(
        "Analyze the following content for any issues:\n\n{}\n\n\
         Check for: harmful content, PII, sensitive information.\
         Return 'PASS' or 'BLOCK: reason'",
        content
    ))
    .with_max_tokens(100);

// The LLM evaluates content semantically
let result = guard.check("...", GuardDirection::Output).await?;
```

---

## GuardManager

```rust
use echo_agent::guard::{GuardManager, GuardDirection};

let mut manager = GuardManager::new();

// Add input guards
manager.add_input_guard(Box::new(injection_guard));
manager.add_input_guard(Box::new(policy_guard));

// Add output guards
manager.add_output_guard(Box::new(pii_guard));
manager.add_output_guard(Box::new(llm_guard));

// Check input
match manager.check_input("User's query").await? {
    GuardResult::Pass => { /* proceed */ }
    GuardResult::Block { reason } => { 
        return Err(Error::Blocked(reason));
    }
    GuardResult::Modify { content } => {
        // Use modified content
    }
}

// Check output
match manager.check_output("Agent's response").await? {
    GuardResult::Pass => { /* return to user */ }
    GuardResult::Block { reason } => { /* redact or error */ }
    GuardResult::Modify { content } => { /* return modified */ }
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
let guard_manager = GuardManager::new()
    .add_input_guard(Box::new(injection_guard))
    .add_output_guard(Box::new(pii_guard));

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
    if content.len() > 10000 {
        Ok(GuardResult::Block { 
            reason: format!("Content too long: {} chars", content.len()) 
        })
    } else {
        Ok(GuardResult::Pass)
    }
}

// Use the generated LengthLimitGuard
manager.add_output_guard(Box::new(LengthLimitGuard));
```

---

## Guard Chaining

Multiple guards are evaluated in sequence:

```rust
manager.add_input_guard(Box::new(guard1));  // First
manager.add_input_guard(Box::new(guard2));  // Second
manager.add_input_guard(Box::new(guard3));  // Third

// Execution:
// 1. guard1.check() → if Block, stop and return
// 2. guard2.check() → if Block, stop and return
// 3. guard3.check() → if Block, stop and return
// 4. All passed → proceed
```

First guard to return `Block` stops the chain.

---

## Built-in Guards

| Guard | Type | Purpose |
|-------|------|---------|
| `RuleGuard` | Rule | Pattern-based blocking |
| `LlmGuard` | LLM | Semantic content analysis |
| `LengthGuard` | Rule | Block oversized content |
| `SecretRedactor` | Rule | Redact secrets with *** |

---

## Best Practices

1. **Layer your guards**: Fast rule-based first, expensive LLM-based last
2. **Be specific with patterns**: Avoid overly broad regex
3. **Log blocked content**: For auditing and tuning
4. **Test thoroughly**: Ensure legitimate content isn't blocked
5. **Consider Modify vs Block**: Sometimes redaction is better than blocking

---

## Performance Considerations

| Guard Type | Latency | Cost |
|------------|---------|------|
| RuleGuard | < 1ms | Free |
| LlmGuard | 100-500ms | API call |

Place LlmGuard at the end of the chain to avoid unnecessary calls.

See: `echo-agent-learning/examples/demo19_guard.rs`