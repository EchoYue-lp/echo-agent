---
schema_version: 1
id: evidence.structured-output-schema-validation-verification
kind: evidence
observed_at: source:0427aee5ee15ea51f4623b1ae3db84522ef774c616f10390c3d7da16064d2ec0
source_refs:
  - src/agent/react/extract.rs
  - src/agent/react/structured.rs
  - src/agent/react/run/stream_channel.rs
  - src/agent/react/run/phases/finalize.rs
  - src/agent/critic/llm_critic.rs
  - echo-agent-learning/examples/demo15_structured_output.rs
supports: [finding.structured-output-schema-validation-contract]
limitations:
  - Remote CI and mainline delivery remain pending
command_results:
  - { command: "cargo test -p echo_agent agent::react::run::stream_channel::tests --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent agent::react::run::phases::finalize::tests --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent agent::react::run::phases::tools::tests --lib --locked", exit_code: 0 }
  - { command: "cargo test -p echo_agent strict_critique_rejects_ --lib --locked", exit_code: 0 }
  - { command: "cargo check -p echo-agent-learning --example demo15_structured_output --locked", exit_code: 0 }
  - { command: "cargo test -p echo-agent-learning --test documentation_contract --locked", exit_code: 0 }
  - { command: "./scripts/verify.sh", exit_code: 0 }
  - { command: "17 independent-feature cargo checks", exit_code: 0 }
---

# Strict structured output verification

## 支持的结论

The final focused stream suite passed 82/82 after cancellation, Stop-hook,
intervention, and schema-order fixes; finalize and tool suites passed 7/7 and
6/6. Red/green regressions cover valid JSON transformed by Output Guard into
invalid JSON, malformed/schema-corrected text and tool answers, multiple
final answers, no-result/unsupported capability, accepted steer, Critic
cancellation without releasing the Critic, managed checkpoint failure reason,
and unissued assistant text excluded from ConversationStore on cancel/block.
Critic strict responses missing required fields or carrying extra fields are
rejected locally.

## 来源与范围

Focused commands inspected real Agent requests, events, trace and paired
FileConversationStore/RuntimeStateStore effects. The last targeted strict
Clippy, formatter and diff checks returned zero. The executable demo15 and
documentation contract compile/test chain passed after injecting LlmConfig.

## 已知缺口

No real provider wire was exercised by these Mock-based regressions. The
full workspace gate and independent feature matrix passed on the final
integrated source. PR CI and mainline strict snapshot remain to be proven.
