# echo-execution

[![crates.io](https://img.shields.io/crates/v/echo_execution?color=brightgreen)](https://crates.io/crates/echo_execution)
[![docs.rs](https://docs.rs/echo_execution/badge.svg)](https://docs.rs/echo_execution)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024%20edition-orange?logo=rust)](https://www.rust-lang.org/)

Execution layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Quickstart

```toml
[dependencies]
echo_execution = "0.1"
```

```rust
use echo_execution::sandbox::LocalSandbox;
use echo_execution::skills::SkillRegistry;
use echo_execution::tools::ToolManager;

// Execute code in a local sandbox
let sandbox = LocalSandbox::new();

// Discover and activate skills
let mut registry = SkillRegistry::new();
registry.discover("./skills")?;

// Register and run tools
let mut tools = ToolManager::new();
tools.register(Box::new(sandbox));
```

## Contents

- **Sandbox**: Code execution in Local / Docker / K8s environments
- **Skills**: Progressive capability disclosure — discover → activate → use
- **Tools**: `ToolManager` — registration, execution, concurrency control

## License

MIT
