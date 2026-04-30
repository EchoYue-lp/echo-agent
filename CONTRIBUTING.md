# Contributing to echo-agent

Thank you for your interest in contributing to echo-agent! This document provides guidelines and instructions for contributing.

## Development Setup

```bash
git clone https://github.com/EchoYue-lp/echo-agent
cd echo-agent
cargo build --workspace
```

## Before Submitting a PR

Please ensure the following checks pass locally:

```bash
# Code formatting
cargo fmt --check --workspace

# Linting
cargo clippy --workspace --all-targets --all-features

# Tests
cargo test --workspace
```

## Commit Messages

- Use the present tense ("Add feature" not "Added feature")
- Use the imperative mood ("Move cursor to..." not "Moves cursor to...")
- Limit the first line to 72 characters or less
- Reference issues and pull requests liberally after the first line

## Pull Request Process

1. Fork the repository and create your branch from `main`
2. If you've added code that should be tested, add tests
3. If you've changed APIs, update the documentation
4. Ensure the test suite passes
5. Make sure your code lints and formats cleanly

## Reporting Issues

When filing an issue, please include:

- A clear and descriptive title
- Steps to reproduce the problem
- Expected vs. actual behavior
- Rust version and operating system
- Relevant code samples or error messages

## Code Style

- Follow the standard Rust naming conventions (<https://rust-lang.github.io/api-guidelines/naming.html>)
- Add doc comments to all public items
- Keep public API stable — deprecate rather than remove when possible
- Write examples using `#![doc = include_str!("../README.md")]` conventions

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
