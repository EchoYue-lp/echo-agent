#!/usr/bin/env bash

set -euo pipefail

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo clippy --workspace --lib --bins --all-features --locked -- \
  -D clippy::unwrap_used \
  -D clippy::expect_used \
  -D clippy::panic \
  -D clippy::unreachable
cargo test --workspace --all-targets --all-features --locked
# --all-targets不运行rustdoc；入门片段必须显式编译，no_run不调用真实模型。
cargo test -p echo-agent-learning --doc --all-features --locked
cargo check --workspace --lib --no-default-features --locked
