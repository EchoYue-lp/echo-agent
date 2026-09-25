# Structured Output

## What It Is

Structured output sends a format hint to the LLM and, for a strict JSON Schema, validates the returned JSON locally before a structured extraction succeeds. Developers no longer need regex or string parsing to deserialize a valid result into a Rust struct.

echo-agent supports structured output at three levels:

```
ResponseFormat (type)
    └─ chat() / stream_chat()               ← raw LLM request
         └─ AgentConfig::response_format()  ← agent-wide config
              └─ ReactAgent::extract_json() / extract::<T>()  ← convenience methods
```

---

## Problem It Solves

### Traditional approach

```
LLM returns: "Person: John Smith, age 34, software engineer"
↓
Developer must: regex / string split / write brittle custom parser
↓
Fragile — breaks whenever the LLM rephrases the output
```

### Structured output approach

```
Define JSON Schema → send provider hint → validate a strict result locally
↓
{"name":"John Smith","age":34,"occupation":"software engineer"}
↓
serde_json::from_str::<Person>() → strongly-typed struct
```

Structured output solves:
- **Information extraction**: pull named fields from unstructured text
- **Classification / labeling**: sentiment analysis, intent detection — constrained enum outputs
- **Format conversion**: turn natural language descriptions into machine-consumable data
- **Batch extraction**: extract array-shaped data from long text (event lists, product catalogs)

---

## Core Types

```rust
/// Response format control
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    /// Default free text
    Text,
    /// Force valid JSON output (no schema validation)
    JsonObject,
    /// Strictly follow a JSON Schema
    JsonSchema { json_schema: JsonSchemaSpec },
}

pub struct JsonSchemaSpec {
    pub name: String,              // schema identifier
    pub schema: serde_json::Value, // standard JSON Schema object
    pub strict: bool,              // enforce strict mode (default: true)
}
```

`ResponseFormat::json_schema()` is the quick-build shortcut:

```rust
let fmt = ResponseFormat::json_schema(
    "person",            // schema name
    json!({              // JSON Schema
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age":  { "type": "integer" }
        },
        "required": ["name", "age"],
        "additionalProperties": false
    }),
);
```

`JsonSchema` with `strict: true` compiles and enforces the schema locally. An invalid schema is rejected before the one-shot LLM request. Invalid JSON and strict schema mismatches share the configured bounded correction retry budget and return typed errors when exhausted. `strict: false` sends only a provider hint; `JsonObject` checks JSON syntax, not schema shape. ADR 0079 rejects external HTTP/file `$ref` targets before the model call; bundle them as local `$defs`.

---

## Usage

### Option 1: `extract_json()` — returns `serde_json::Value`

Best when you need dynamic field access or don't want to define a Rust struct:

```rust
use echo_agent::prelude::*;
use serde_json::json;

let llm_config = LlmConfig::for_provider(
    "openai",
    "https://api.openai.com/v1",
    std::env::var("OPENAI_API_KEY")?,
    "gpt-5.5",
    LlmApiProtocol::Responses,
)?;
let agent = ReactAgentBuilder::new()
    .llm_config(llm_config)
    .name("extractor")
    .system_prompt("You are a precise information extractor")
    .disable_cot()
    .build()?;

let schema = ResponseFormat::json_schema(
    "person",
    json!({
        "type": "object",
        "properties": {
            "name":       { "type": "string" },
            "age":        { "type": "integer" },
            "occupation": { "type": "string" }
        },
        "required": ["name", "age", "occupation"],
        "additionalProperties": false
    }),
);

let value = agent.extract_json(
    "John Smith, 34, works as a software engineer at a tech company in Seattle.",
    schema,
).await?;

println!("{}", value["name"]);       // "John Smith"
println!("{}", value["age"]);        // 34
println!("{}", value["occupation"]); // "software engineer"
```

### Option 2: `extract::<T>()` — deserializes directly into a Rust struct

The most ergonomic approach — type-safe, compile-time checked:

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SentimentResult {
    sentiment:  String,    // "positive" | "negative" | "neutral"
    confidence: f64,       // 0.0 ~ 1.0
    keywords:   Vec<String>,
    summary:    String,
}

let schema = ResponseFormat::json_schema(
    "sentiment_result",
    json!({
        "type": "object",
        "properties": {
            "sentiment":  { "type": "string", "enum": ["positive", "negative", "neutral"] },
            "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
            "keywords":   { "type": "array", "items": { "type": "string" } },
            "summary":    { "type": "string" }
        },
        "required": ["sentiment", "confidence", "keywords", "summary"],
        "additionalProperties": false
    }),
);

let review = "This phone is absolutely amazing! Performance is blazing fast, battery lasts all day. Highly recommended!";
let result: SentimentResult = agent.extract(review, schema).await?;

println!("Sentiment:   {}", result.sentiment);                  // "positive"
println!("Confidence:  {:.0}%", result.confidence * 100.0);     // "96%"
println!("Keywords:    {:?}", result.keywords);
```

### Option 3: `ReactAgentBuilder::response_format()` — agent-wide config

Stores the Agent-wide format in the run snapshot and sends JSON formats on every main ReAct request. The resolved model profile must affirm structured-output support; unknown or unsupported models fail before the model call. Strict JSON Schema is also validated locally before either text or `final_answer` tool output can become a successful final answer. JSON and schema failures receive bounded repair attempts; exhaustion fails the run. `execute_typed()` additionally deserializes the accepted value into the requested Rust type:

```rust
let llm_config = LlmConfig::for_provider(
    "openai",
    "https://api.openai.com/v1",
    std::env::var("OPENAI_API_KEY")?,
    "gpt-5.5",
    LlmApiProtocol::Responses,
)?;
let mut agent = ReactAgentBuilder::new()
    .llm_config(llm_config)
    .name("translator")
    .system_prompt("You are a translation assistant")
    .response_format(ResponseFormat::json_schema(
        "translation_result",
        json!({
            "type": "object",
            "properties": {
                "original":    { "type": "string" },
                "translation": { "type": "string" },
                "language":    { "type": "string" }
            },
            "required": ["original", "translation", "language"],
            "additionalProperties": false
        }),
    ))
    .disable_cot()
    .build()?;

let v: serde_json::Value = agent.execute_typed("Artificial intelligence is transforming the world.").await?;
println!("translation: {}", v["translation"]);
```

### Option 4: Arrays and nested structures

Extract multiple records from long text:

```rust
#[derive(Debug, Deserialize)]
struct EventList {
    events: Vec<HistoryEvent>,
}

#[derive(Debug, Deserialize)]
struct HistoryEvent {
    year: i32,
    description: String,
    significance: String,
}

let schema = ResponseFormat::json_schema(
    "event_list",
    json!({
        "type": "object",
        "properties": {
            "events": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "year":         { "type": "integer" },
                        "description":  { "type": "string" },
                        "significance": { "type": "string" }
                    },
                    "required": ["year", "description", "significance"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["events"],
        "additionalProperties": false
    }),
);

let result: EventList = agent.extract(long_text, schema).await?;
for event in &result.events {
    println!("[{}] {} — {}", event.year, event.description, event.significance);
}
```

---

## Mode Comparison

| Mode | Use case | Schema validation |
|------|----------|-------------------|
| `ResponseFormat::Text` | Default, free-form Q&A | None |
| `ResponseFormat::JsonObject` | Any JSON output, fields not fixed | Valid JSON only |
| `ResponseFormat::JsonSchema` | Fixed-field extraction / classification / conversion | Local schema validation when `strict: true`; provider hint only when `strict: false` |

---

## Relationship to Tool Calls

`extract_json()` / `extract()` **bypass the ReAct loop entirely** — no tool calls, no iterations:

```
extract_json(prompt, schema)
    │
    └─ chat() call with response_format
         LLM outputs JSON text
         parse, validate strict schema, or retry invalid JSON/schema mismatch within the configured bound
```

To combine tool-based data gathering with structured output, use a two-phase pattern:

```rust
// Phase 1: ReAct Agent collects data with tools
let raw_answer = agent.execute("Query the last 3 days of sales and summarize").await?;

// Phase 2: Structured extraction from the gathered data
let report: SalesReport = extractor_agent.extract(&raw_answer, schema).await?;
```

---

## Important Notes

1. **Model compatibility**: JSON response formats require a fresh, affirmative structured-output capability. Provider support helps produce valid output, but the local strict validator rejects a mismatch even if a provider ignores the hint. An explicit `Text` format is sent as the unconstrained provider default.
2. **`additionalProperties: false`**: Always set this in your JSON Schema to prevent the model from emitting extra fields
3. **CoT**: Extraction tasks generally don't need chain-of-thought — use `.enable_cot(false)` to avoid interference
4. **Temperature**: `extract_json()` internally uses `temperature=0.0` for stable outputs. The `AgentConfig::response_format()` path uses the Agent's configured temperature
5. **Schema references**: Strict validation supports local references such as `$defs`; external HTTP/file `$ref` targets are rejected before the model call. Bundle external definitions into the supplied schema.

---

## Full Example

See: `echo-agent-learning/examples/demo15_structured_output.rs`

```bash
cargo run -p echo-agent-learning --example demo15_structured_output
```
