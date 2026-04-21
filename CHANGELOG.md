# Changelog

All notable changes to this project will be documented in this file.

## v1.4.0

### Highlights

- Hardened runtime contracts across sandbox, human-loop, memory, and file-based skills.
- Reworked examples into clearer acceptance tiers and tightened many examples into fail-fast validation surfaces.
- Aligned docs and runtime behavior so documented constraints now more closely match real enforcement.
- Unified all workspace crates to version `1.4.0`.

### Changed

- Refined memory/storage boundaries across `Checkpointer`, conversation history, store, and embedding-related layers.
- Updated file-skill activation so `paths` constraints require a matching `context_path` at runtime.
- Added runtime `allowed-tools` enforcement for built-in skill tools such as `read_skill_resource` and `run_skill_script`.
- Stabilized hook behavior, environment propagation, and execution ordering for file-based skills.
- Integrated sandbox-aware execution more consistently across skill and shell-related paths.

### Examples

- Added `examples/README.md` to classify examples as acceptance, conditional acceptance, or teaching examples.
- Tightened many examples to fail loudly on missing prerequisites or broken core functionality instead of silently degrading.
- Improved approval, compression, external-skill, sandbox, MCP, memory, and provider demos to better serve as validation surfaces.

### Docs

- Updated English and Chinese docs for memory semantics, human-loop behavior, skill activation, and runtime guarantees.
- Reduced overclaiming in skill/tooling docs where behavior is prompt-guided versus runtime-enforced.

### Validation

- Verified the release with workspace `cargo check`.
- Ran targeted and full test coverage during the implementation work, including `echo_execution` and `echo_agent` suites.

