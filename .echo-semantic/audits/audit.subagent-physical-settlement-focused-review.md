---
{
  "schema_version": 1,
  "id": "audit.subagent-physical-settlement-focused-review",
  "kind": "audit",
  "boundary_ref": "boundary.task-subagent-workflow",
  "lens": "time_lifecycle",
  "freshness": "examined",
  "revision": "source:f8a94d2c787deeda1eab3361e4993ccf20cb7fd2b4d925fbb1b1a12719c45360",
  "finding_refs": [
    "finding.subagent-physical-settlement-observation"
  ],
  "challenges": {
    "original-owner-and-typed-uncertainty": {
      "revision": "87223c8163f6b30479fbd44eadd09736db50bd5d",
      "source_refs": [
        "echo-core/src/agent/admission.rs",
        "echo-core/src/agent/mod.rs",
        "echo-core/src/agent/event_envelope.rs",
        "echo-core/src/error.rs",
        "echo-orchestration/src/runtime/turn_driver.rs",
        "echo-orchestration/src/tasks/runtime_service.rs",
        "echo-orchestration/src/tasks/runtime_executor.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "react-subagent-resources": {
      "revision": "87223c8163f6b30479fbd44eadd09736db50bd5d",
      "source_refs": [
        "src/agent/react/lifecycle.rs",
        "src/agent/react/run/stream_channel.rs",
        "src/agent/steer.rs",
        "src/agent/subagent/executor.rs",
        "src/agent/subagent/team/mod.rs",
        "src/acp/runtime.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "canonical-tool-physical-settlement": {
      "revision": "87223c8163f6b30479fbd44eadd09736db50bd5d",
      "source_refs": [
        "echo-execution/src/tools.rs",
        "echo-tools/src/code.rs",
        "src/agent/snapshot.rs",
        "src/agent/react/run/pipeline.rs",
        "src/agent/react/run/phases/tools.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "session-close-observation": {
      "revision": "ff0e698e69746a8b55cc2261d0f678094f4bca4e",
      "source_refs": [
        "src/acp/session.rs",
        "docs/adr/0088-subagent-settlement-observation.md"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "headless-original-owner": {
      "revision": "source:f8a94d2c787deeda1eab3361e4993ccf20cb7fd2b4d925fbb1b1a12719c45360",
      "source_refs": [
        "src/headless.rs",
        "echo-agent-learning/tests/example_contracts/demo54_headless.rs",
        "docs/adr/0088-subagent-settlement-observation.md"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    }
  }
}
---

# Focused physical settlement implementation review

## 审查范围

Independent reviewer settlement_b_final_source_review returned PASS, Critical 0,
Important 0, Minor 0. It checked the fixed source/test/maintained-document
identities and the relevant source paths plus bounded implementation increments;
every file in each reviewed freeze matched its stated identity.
Official Plan validation passed. Original owner, typed uncertainty, resource
retention, raw EOF, InputReceipt, exact claim CAS and canonical Tool paths were
covered by the original review and its bounded increments.

## 实际实现路径与证据

The reviewer read actual Manager 335, React run 189, Subagent executor 69 and
RunCode 16 results, receipts and unchanged 581-input guards. Compiler diagnostics
were zero. Collections are not cumulative unique coverage. Receiver-closure
cases had genuine before exit 101 and same-case after 0. Original producer/finally,
file effects, permit and source-drop assertions were checked. Three real Task
cases retained the exact claim through sequential/concurrent default five-second
grace and the original 270ms batch budget. These are after tests, not individually
claimed original-source counterexamples.

## 已检查故障假设

Controlled intervention uses explicit Notify completion while preserving original
Skipped/audit/redaction/completed-peer/trajectory assertions. Four pre-execution
cancellation guards reuse the original interruption projector; typed unknown and
already-known results are not replaced.

## 问题记录

Earlier Core/Orchestration results retain their actual source cuts. Overwritten
early raw, fixture errors, compilation failures and zero-case selections are not
borrowed as passes. Raw stays task-local, not bundled into the repository.

## 未检查项

The original 39/40-file focused review supported development milestones and did
not claim full gates. Current six-gate execution is recorded in the later
Evidence/Audit increment below. The Finding remains open for the live-source observation-to-terminal gap and
SDK/CLI consumer/exact merged-main qualification.

## 残余风险

Caller-provided
opaque resource graphs require consumer qualification: internal retention having
no self cycle does not prove arbitrary external Any values cycle-free.

## ACP close独立增量复审

同一独立reviewer在87223c8后读取唯一生产session.rs及三规范增量，精40文件当前字节全match，C0/I0/M0。核查私有Some/None观察、同generation、无重发、原Notify等待、无synthetic身份/新owner task，close_all真实join所有entry后才typed返回，以及ordinary Failed的新generation retry与真正Settled指针限定清理不变。两typed入口与无identity loss的原before均真101；全原Session18after0、2160B、581输入前后相同、零warning、fmt与Plan0已读取。前面三个challenge绑定已推送87223c8的原source，不重标原工程执行时间或声称这些原集合在新cut重跑。六门禁/17feature/consumer/main仍待验。

## Headless与最终门禁增量

同一独立reviewer核Headless真实file producer反例与1183B/101、修后全11/0/0/1322B/581无漂移/零warning；私owner同handle、state内owner-free、无环、原5公开值/Debug/格式保持、6外crate逐值迁移均PASS C0/I0/M0。原after2源码与最终源码生产部分完全同，最终仅while追加两析构尾条件；该最窄增强保2s及所有physical断言，最终full05已实际覆盖。三Clippy等价增量也有独立C0/I0/M0，不新增owner或锁范围。

当前六原门禁实际全部0，完整测试3580主通过/0失败/3原opt-in忽略，另两child不重复加总。每gate581输入无漂移，日志完整，JOBS1/debug0/strip symbols/inc0；具体身份与执行切点见同Evidence。17独立feature也已actual0/零warning/逐581输入无漂移，consumer与精确main CI仍未借用。原39/40独审历史正文保持当时范围，不把旧声明冒充新main；本增量刷新Audit当前源码身份，session历史挑战固定真实已推送ff0，原872三个挑战保原历史。

## 本checkpoint审查与后继边界

23本地门禁已通过，只支持当前工程资格。Headless、三Clippy等价短路与三旧fixture校正的限定源码增量各有C0/I0/M0独审；这不是对live-source已识别的sticky observation路径出具无缺陷结论。该路径尚待真实反例与同owner/gen合法terminal+EOF闭合证明，宽Finding仍open。其余原未检查路由和跨仓组合限制不变。
