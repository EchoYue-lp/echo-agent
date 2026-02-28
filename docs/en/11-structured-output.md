# Structured Output

## What It Is

Structured output forces the LLM to return JSON that strictly conforms to a **pre-defined JSON Schema**, instead of free-form text. Developers no longer need regex or string parsing — the LLM's response can be deserialized directly into a Rust struct.

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
Define JSON Schema → pass to LLM → LLM outputs strictly
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

---

## Usage

### Option 1: `extract_json()` — returns `serde_json::Value`

Best when you need dynamic field access or don't want to define a Rust struct:

```rust
use echo_agent::prelude::*;
use serde_json::json;

let config = AgentConfig::new("gpt-4o", "extractor", "You are a precise information extractor")
    .enable_cot(false);  // no reasoning chain needed for pure extraction
let agent = ReactAgent::new(config);

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

### Option 3: `AgentConfig::response_format()` — agent-wide config

Forces every LLM call made by this Agent to use the specified format. Best for a dedicated "extraction agent":

```rust
let config = AgentConfig::new("gpt-4o", "translator", "You are a translation assistant")
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
    .enable_cot(false);

let mut agent = ReactAgent::new(config);

// execute() returns a JSON string directly
let raw = agent.execute("Artificial intelligence is transforming the world.").await?;
let v: serde_json::Value = serde_json::from_str(&raw)?;
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
| `ResponseFormat::JsonSchema` | Fixed-field extraction / classification / conversion | Strict schema |

---

## Relationship to Tool Calls

`extract_json()` / `extract()` **bypass the ReAct loop entirely** — no tool calls, no iterations:

```
extract_json(prompt, schema)
    │
    └─ single chat() call with response_format
         LLM outputs JSON text
         parse and return immediately — no ReAct iterations
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

1. **Model compatibility**: `JsonSchema` strict mode requires the model to support Structured Outputs (e.g., GPT-4o, Qwen3). For unsupported models, fall back to `JsonObject`
2. **`additionalProperties: false`**: Always set this in your JSON Schema to prevent the model from emitting extra fields
3. **CoT**: Extraction tasks generally don't need chain-of-thought — use `.enable_cot(false)` to avoid interference
4. **Temperature**: `extract_json()` internally uses `temperature=0.0` for stable outputs. The `AgentConfig::response_format()` path uses the Agent's configured temperature

---

## Full Example

See: `examples/demo15_structured_output.rs`

```bash
cargo run --example demo15_structured_output
```
