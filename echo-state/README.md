# echo-state

State management layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Contents

- **Memory**: Dual-layer memory — `Store` (long-term KV) + `Checkpointer` (session persistence)
- **Context Compression**: SlidingWindow, LLM Summary, and Hybrid compressors
- **Audit Logging**: Structured event logging with pluggable backends
- **SQLite persistence**: Optional `sqlite` feature for disk-backed memory

## Feature Flags

- `sqlite` — Enable SQLite-backed memory storage

## Usage

```rust
use echo_state::memory::InMemoryStore;
use echo_state::compression::SlidingWindowCompressor;
```

## License

MIT
