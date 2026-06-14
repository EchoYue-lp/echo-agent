# echo-state

[![crates.io](https://img.shields.io/crates/v/echo_state?color=brightgreen)](https://crates.io/crates/echo_state)
[![docs.rs](https://docs.rs/echo_state/badge.svg)](https://docs.rs/echo_state)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

State management layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

```toml
[dependencies]
echo_state = "0.2"
```

```rust
use echo_state::memory::InMemoryStore;
use echo_state::compression::SlidingWindowCompressor;
use echo_state::audit::InMemoryAuditLogger;

// Persistent key-value memory
let store = InMemoryStore::new();

// Context window compression
let compressor = SlidingWindowCompressor::new(4096);
```

## Contents

- **Memory**: `Store` (long-term KV) + `ConversationStore` (transcript persistence)
- **Context Compression**: SlidingWindow, LLM Summary, and Hybrid compressors
- **Audit Logging**: Structured event logging with pluggable backends

## Feature Flags

| Flag | Description |
|------|-------------|
| `sqlite` | Enable `SqliteStore` for disk-backed persistent memory |

## License

MIT
