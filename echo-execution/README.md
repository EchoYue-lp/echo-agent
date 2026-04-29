# echo-execution

Execution layer for the [echo-agent](https://crates.io/crates/echo_agent) framework.

## Contents

- **Sandbox**: Code execution in Local / Docker / K8s environments
- **Skills**: Progressive capability disclosure — discover → activate → use
- **Tools**: Built-in tools for shell, file, and script execution

## Usage

```rust
use echo_execution::sandbox::LocalSandbox;
use echo_execution::skills::SkillRegistry;
```

## License

MIT
