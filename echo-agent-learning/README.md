# echo-agent-learning

`echo-agent-learning` is the non-published learning and example package for
the `echo-agent` framework. It combines the Rust lessons, numbered `demo_*.rs`
walkthroughs, comprehensive examples, and deterministic public-facade
contracts in one place. It is not part of the production runtime.

The package depends on `echo_agent` through its public facade. It is therefore
useful both for new contributors learning the framework and for checking that
the documented public API remains usable by an external consumer.

## Start Here

From the `echo-agent` repository root:

```bash
cargo test -p echo-agent-learning
cargo run -p echo-agent-learning --example chapter_01_basics
cargo run -p echo-agent-learning --example demo00_quickstart --locked
cargo run -p echo-agent-learning --example demo01_tools --locked
```

The chapter examples are offline and need no model or network. Numbered demos
are progressively ordered but may require a feature, local service, credential,
or provider. Each demo documents its own prerequisites at the top of the file.

## Learning Route

Read [`docs/zh/README.md`](docs/zh/README.md) for the complete fifteen-chapter
Rust course. The course covers Cargo/workspaces, ownership, error handling,
traits and macros, async execution, serialization, testing, and how to read a
real `echo-agent` tool path.

Then run the numbered demos in increasing order. The early demos introduce
tools, tasks, streaming, memory, workflows, and structured output. Later demos
combine those capabilities with MCP, channels, sandboxes, Git worktrees,
research, data processing, and plugin systems.

## Examples

All existing numbered demos remain learning-oriented source files under
[`examples/`](examples/). New comprehensive walkthroughs should be added there
with a descriptive `comprehensive_*.rs` name and a clear prerequisite section.

The example disposition and maintenance rules are documented in
[`examples/README.md`](examples/README.md). Deterministic contracts that do not
have a `main` function live under
[`tests/example_contracts/`](tests/example_contracts/) and run through the
shared [`tests/example_contracts.rs`](tests/example_contracts.rs) harness.

## Validation

```bash
cargo check -p echo-agent-learning --all-targets --all-features --locked
cargo test -p echo-agent-learning --all-features --locked -- --test-threads=1
cargo clippy -p echo-agent-learning --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
```

The learning package follows the repository rules: no unchecked panic paths,
no UTF-8 byte slicing, no unbounded external output, and no framework-internal
or EKO application imports. Framework API tests remain in the parent package;
this crate tests the consumer boundary.

## Layout

```text
echo-agent-learning/
├── src/                         Rust lesson implementations
├── docs/zh/                     Rust and framework learning guides
├── examples/                    chapter examples and demo_*.rs walkthroughs
├── tests/example_contracts/     deterministic public-facade contracts
├── tests/learning_contract.rs   lesson-level contracts
└── src/bin/facade_consumer.rs   external consumer facade probe
```
