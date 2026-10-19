---
schema_version: 1
id: evidence.model-facts-authority-verification
kind: evidence
observed_at: 07e4270380c40df3f412f99c0aecb6145410cb2a
source_refs:
  - src/agent/react/capabilities.rs
  - src/agent/react/tests.rs
  - echo-core/src/llm/capabilities.rs
  - echo-core/src/llm/mod.rs
  - echo-integration/src/providers/config.rs
  - echo-integration/src/providers/mod.rs
  - echo-integration/src/providers/openai.rs
  - echo-integration/src/providers/responses.rs
  - echo-integration/src/providers/anthropic.rs
  - src/config.rs
  - src/lib.rs
  - src/llm.rs
  - src/agent/react/builder.rs
  - src/agent/react/mod.rs
  - src/agent/react/run/phases/prepare.rs
  - src/agent/snapshot.rs
  - docs/en/38-factory-modes.md
  - docs/zh/38-factory-modes.md
supports: [behavior.llm-provider-execution, rule.provider-protocol-boundary, finding.runtime-model-client-publication, finding.runtime-token-limit-explicit-publication]
limitations:
  - 完整workspace all-feature门禁、SDK contract生成与远端CI按父任务要求留到集成分支
  - shared semantic baseline和全局source digest未在并行worktree刷新
---

# Model facts authority 验证证据

## 支持的结论

严格档 red test 先覆盖 ModelFact 类型、freshness API 和 conservative capability 的缺失路径；当前 worktree 已建立针对五层 precedence、source normalization、future/expired/empty、built-in observation time、历史 provider label compatibility、unknown provider/model isolation、Anthropic custom-label capabilities、capability/profile 不变量、tokenizer receipt、serde、protocol baseline 和 runtime snapshot freshness 的 focused 覆盖。tokenizer dispatch 本身不在本切片范围，留给 #100。

`echo_integration providers::` 的 focused 覆盖包含 SourcedLlmConfig roundtrip、legacy LlmConfig decode、fresh/stale thinking 和 budget facts、partial provider 与三个 adapter 固定 protocol baseline 及 provider label 隔离。root config 和 snapshot/profile focused tests 覆盖 SourcedModelConfig、legacy context provenance、过期窗口降级、跨 identity 隔离、caller 重放、RuntimeConfig budget 重建、prepare safe point 和 Tool policy；这些结果需要在最新修复（含 Anthropic adapter 与显式 flag precedence）后重新取得。

`cargo fmt --all -- --check` 与 `git diff --check` 在当前改动后通过；受影响 package 的 focused Clippy 仍待空间恢复后重新执行。

## 来源与范围

命令receipt由本轮执行器捕获并在父任务handoff中逐项报告；验证仅覆盖本切片受影响packages和行为，不替代父任务合并到main或发起MR前的完整门禁。

## 已知缺口

独立rereview单独记录；并行分支合并后必须统一刷新semantic baseline/source引用并生成SDK contract artifacts，再执行完整门禁。

## Runtime model publication counterexample (2026-10-19)

The independent native SDK before observation used the original immutable Host
SHA256 `0982121cb1e99284b4a7d7e07af35037f9f22bd653528d9716d425f5609407a0`.
Two completed original runs on the same Agent/Session generated real HTTP models
`[memory-model, memory-model]` around a requested switch to `native-next-model`.
The native log was 1,621 bytes, SHA256
`6f34ab2207e7dbb2115114aaefe229257ae41c334f5e2b7addff61131b75d72d`.
Its raw receipt remains local to the SDK task's
`.echo-semantic/local/sdk-parallel-delivery-01a14385/accepted-live-model-http-before-proof.json`;
it is neither copied into Git nor represented as a newly executed framework test.

The framework independently ran the same-Agent concrete-adapter HTTP regression
against unchanged `e3ba883a9abaf4e7070185304ccb676c55292144` production code: both
turns completed and the real request models were `[model-a, model-a]`, producing
a natural assertion failure (exit 101). The identical selected HTTP case and
argv passed after repair (exit 0, `[model-a, model-b]`). Its case bytes were
identical before formatting, SHA256
`781b312bf5a9d911f29d7989ac3683a6a16493cb174a7f9dd7d2497e1a1ab4a5`.
Local originals belong under `.echo-semantic/local/live-model-publication/`: the
before log is 1,045 bytes, SHA256
`b2c2832e8df5f9d4d03dec5893c47f2c3e2c697c8d3c25a780b2be18acd1b4c3`;
the after log is 524 bytes, SHA256
`f933fded19ac8468daf35344034c6f2934c693a96dc584011c73a0216749b515`.
Result JSON records actual argv, source identities, environment, resource
observations, natural exits and log identity; files are local-only originals.

After formatting, seven runtime-model tests passed: original HTTP/history, custom
injection through both APIs with typed refusal and rollback, consistent no-op and
invalid-name/header rollback, Self builders and fallible builder, fresh/expired
sourced precedence with retained complementary facts, implicit/explicit budget
following actual A/B turns on the same Context, and prepared profile queries
occurring during prepare but never commit/drop. Four existing explicit-limit
regressions and one configuration-only setter regression passed. Ten matching
prepared tests passed, including two tests already counted above; these are not
ten additional new scenarios. Package lib/tests Clippy completed with zero
warnings. All Rust checks used one Cargo process, jobs=1, incremental=0 and
resource-only debug-info=0, retaining debug assertions, overflow checks and unwind.
Independent final framework source rereview returned C0/I0/M0 after the
channels correction; this does not qualify the SDK construction/getter changes. A boxed AgentHandle wrapper's cached borrowed model string
is a separate conversion snapshot; native HTTP repair alone cannot prove the SDK
Session getter has been corrected. Framework full gates, exact-main CI, SDK pin,
generation and refreshed consumer tests remain later delivery obligations.


The installed 0.5 task reader limitation already recorded by
`evidence.native-value-semantic-history-reader-limit` applies to its historical
SDK references. This task initially reported 618 console errors, including two
new Finding heading errors corrected to the required heading. A later complete
raw task invocation exited 1 with 629 diagnostics: 607 current-tree reads of
extracted historical SDK paths, one bounded-closure limit and 21 unloaded-object
reference reports; all nine distinct reported reference targets exist with their
correct object kinds. Raw log/result/classification stay ignored in this task
directory. No historical SDK path or object relationship was removed. The task
command is not reported as passing, and its count is not substituted for the
historical 614. The current official full repository strict snapshot/change-
evidence check must resolve actual historical Git; it does not by itself prove
a native task Stop or any business-test exemption.


The first independent source review found one required conditional-caller fix:
`src/channels.rs` still invoked the old private installation signature, outside
the default-feature focused build. Its configured factory now preserves the
original single client build before Agent construction, then prepares the model
name/profile and invokes the same pure publisher. The preparation helper is
crate-private, with no new public API or channel product policy. The actual
`cargo check -p echo_agent --no-default-features --features channels --locked`
passed after this fix (raw `channels-check-result.json` in the ignored task
directory). The final five-source model-focused rerun passed all seven cases after this
correction; the remaining matching focused and Clippy receipts are recorded
separately on those same identities; the isolated feature check does not
replace the later complete feature matrix.

The current full strict history check also exposed two older Graph observation
markers that only named `source:5d89e64c394894674208e32f5978a76c16e4c26081038a4378fe62dccf16908d`.
The official checker's `source_digest_at_revision` proves that digest is exactly
main ancestor `f3fa0705f62fe813a5a86bc41cd673f82f915a61` (and not its6563 parent).
Only those two markers in the existing workflow negative-control Evidence and
focused settlement Audit are rebound to f3fa; their referenced historical blobs,
text, test totals and scope are unchanged. This preserves old proof after the
current source changes instead of relabeling it as a new current execution.
