---
{
  "schema_version": 1,
  "id": "audit.subagent-physical-settlement-focused-review",
  "kind": "audit",
  "boundary_ref": "boundary.task-subagent-workflow",
  "lens": "time_lifecycle",
  "freshness": "examined",
  "revision": "875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c",
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
      "revision": "f0a6cd1c13706c20955209b698c1f891c09d78b3",
      "source_refs": [
        "src/headless.rs",
        "echo-agent-learning/tests/example_contracts/demo54_headless.rs",
        "docs/adr/0088-subagent-settlement-observation.md"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "live-source-late-settlement": {
      "revision": "ffff6101be6adc8f06351987e8c4166fd3bd98bd",
      "source_refs": [
        "echo-core/src/agent/mod.rs",
        "echo-orchestration/src/runtime/turn_driver.rs",
        "src/agent/subagent/executor.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "scheduler-original-occurrence": {
      "revision": "3dedb9a6834cd28a3718d9852023722a6d24f4dc",
      "source_refs": [
        "echo-orchestration/src/scheduler/runner.rs",
        "echo-state/src/delivery.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "workflow-protected-original-owner": {
      "revision": "663cf644ab6ad7b039366529bdd0e5837d922cf5",
      "source_refs": [
        "echo-orchestration/src/workflow/graph.rs",
        "echo-orchestration/src/workflow/checkpoint_store.rs",
        "echo-orchestration/src/workflow/node.rs",
        "echo-core/src/agent/mod.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "workflow-guide-protection-contract": {
      "revision": "875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c",
      "source_refs": [
        "docs/en/17-graph-workflow.md",
        "docs/zh/17-graph-workflow.md",
        "echo-agent-learning/tests/documentation_contract.rs"
      ],
      "evidence_refs": [
        "evidence.subagent-physical-settlement-repair"
      ]
    },
    "graph-foreign-and-latest-observation-controls": {
      "revision": "source:77d8b6e0696f0e1fc7fd8a486a2aa6e7ffcce0df90bf331e5ca39da934ef0e47",
      "source_refs": [
        "echo-orchestration/src/workflow/graph.rs",
        "echo-orchestration/src/workflow/node.rs",
        "echo-orchestration/src/workflow/checkpoint_store.rs"
      ],
      "evidence_refs": [
        "evidence.workflow-observation-negative-controls"
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

## Live-source有限实施增量

独立reviewer settlement_b_final_source_review对七路径freeze2 d06621ebf5e6385b8bab5cc499b219ae35da2a4c0eb149686a4fcd64a19dd768核当前字节，PASS C0/I0/M0。核原rawtap在normalizer前、精确身份与latestUnknown顺序、foreign保留、sink独立、Core取消终态在真实EOF后、input原watch及Team原claim/active/effect闭包。真正before2903B/101、同caseafter0和Core426/Orch477/Executor70实际0、每581guard与0warning均已读取；测试后仅等价两个matchguard和一个tailif折叠，currentClippy2/fmt2实际0亦已核。

原Headless challenge固定真实已公开f0a6，23原门禁仍属于该历史切点。此增量不是重新全库逐字审或当前full6/17资格。Scheduler与Workflow两个实际消费者的typed观察/重排/owner-loss后继未实施，旧普通重试义务不删除；消费者和main资格仍未闭。

## Scheduler有限实施增量

独立reviewer settlement_b_final_source_review复核Scheduler全部相关入口及本次有界增量，最终C0/I0/M0，绑定六路径freeze2 5836dad5e3dba290d0edefef8d4cf635dcf4acb1f8d54d0cbe6793e053cecf28、源码patch71d4198c92e8951d1fa40d1ea7bd672cae4be23b347c1efa89c0bb0a6b4c1a1b。原Handle parent/StrongWeak、factory与poll panic、borrowed waiter、同journal保护、legacy当前attempt升级、exactclaim原子替换与retained_bytes/terminal retention、None旧字节与Some旧decoder拒绝均已审。最后I1两gate只作用None，Some等同原completion；真实poison与cancel反例after、whole53、fmt/Clippy及581guard均已实际读取。误报的pre-effect Claimed永久卡已沿canonical selector撤回，没有据此增加实现。

ADR中Workflow A已接受但尚未本增量实施的措辞已核；不是另一个待用户选择。此次审查不重宣全仓没有缺陷或最终23门禁已覆盖当前新代码。原live-source challenge固定ffff6101，历史23固定f0a6；Workflow A、跨仓消费者与merged-main资格仍开放。

## Workflow A有限实施增量

NativeStore由sdk_archive_role_review独立源码/原件专审C0/I0/M0，绑定原生产d58e39b2及实际旧reader proof2；当前相对该cut仅实际protect bytes拒原wrapper/bare两decoder断言，生产不变且原Store16再次0。旧File模块来自公开3ded，probe只是该模块/未变共享类型与精依赖闭包，不是旧完整框架binary；四双进程同fd实际WouldBlock后才放行。最早sleep-only竞争结论已由更强proof2替代，不冒实际掉电或旧delete/clear安全。

Graph/Core由settlement_b_final_source_review对相关原路径与有界增量独审，最终C0/I0/M0，精九文件freeze3 1f80e51b457ed072a6620369c65bfe594c618418c0390db359bc33cd5d990a53。核唯一owned原loop、caller/firstyield Drop、fanout panic保同siblings、legacy chat raw adapter、won claim身份与knownattempt、matching source terminal/EOF及Store知识分离。typed renew/requeue真实红绿、ordinary aux诊断不改首cause已核；Core427/33575B按不变源复用，当前Orch508/45195B、Clippy227B、fmt0B实际0，逐581输入/零warning/无截断均已读。

当前只支持必要开发checkpoint。Graph-specific foreign/lateUnknown的专例未在本切片单独执行；Core/Driver旧negative不充作其新运行。历史23固定f0a，Scheduler challenge固定3ded；新源码最终6+17、SDK注册Store/三语Host、CLI消费与精确main CI仍待，Finding open、Plan ready。全部raw、probe源码和候选留local，没有原件包进入Git。

## 最终23与双语指南有界增量

同settlement_b_final_source_review逐原件核663的六门禁/17feature及581输入，actual全0、无warning/截断；05独算主3620/0/3、两child不叠。旧9source仍精freeze3，未重新逐字审全仓；原bounded实施结论继续适用。当前仅两guide checkpoint章节新增，固定manifest fdbcec48e7bc39e696b259d84a26fabcd902b0144b83ba32791b94090d714c68与patch/copies/current字节一致，规范/API/限制和demo39链接核实，BeforeNode/finish及其它正文未变。

reviewer实际读取PF纠正链：有效宽PF两路径allow+mutable，check0→restore精663→前置check0→同candidate重放→同full05原binary读取新文档contract2 1pass/193B0→finalcheck0；9source/581无变，首次错误与旧PFmutable证据缺口未追认为通过。内容、执行输入桥及该限定流程收口C0/I0/M0。当前Source摘要是在指南改后观测，23仍固定663旧文档epoch；不是对新指南倒填全23执行。SDK/CLI、精确main CI及最终交付仍待。


历史快照固定：后继ZIP依赖修复开始时，将原source:736ac2观测固定到875cc22f；canonical对该Git树实算源码digest仍精等736ac2。此为同一旧源码/文档的可恢复定位，不新增B测试、复审或mainCI通过声明。

## Graph foreign 与 latestUnknown 具名负例补证

accepted6563上仅扩现有测试fixture，生产Graph/Node/Store不变。两个具名负例分别验证foreign→matching→localterminal→EOF仍保原foreign身份、matching→terminal→latermatchingUnknown→EOF仍保最新观察；同原claim不可重领，实际resumed内存及MemoryStore snapshot均无结果/assistant提交，calls1/drop1。原matching正控路径不变。新五控5/0/0及workflow子集93/0/0均实际exit0、零warning/源码漂移；精源码与完整raw摘要见evidence.workflow-observation-negative-controls。

此处补足原审查206所述两项Graph专例未执行的局部覆盖，不把旧历史限制重写成当时已通过，不宣File/重启/remoteStore新证明。独立候选与执行结果复审均C0/I0/M0，原Finding继续open；新源码完整合并门禁、SDK/CLI消费者及精main资格仍独立验收。
