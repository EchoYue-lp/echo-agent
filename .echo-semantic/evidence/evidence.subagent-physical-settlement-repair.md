---
{
  "schema_version": 1,
  "id": "evidence.subagent-physical-settlement-repair",
  "kind": "evidence",
  "observed_at": "source:951cf5e91f3d0fb879bda5e1960e21750accb35acdc3246c14fb6a900b53225e",
  "source_refs": [
    "docs/adr/0088-subagent-settlement-observation.md",
    "docs/subagent-cancellation-observation.md",
    "echo-core/src/agent/admission.rs",
    "echo-core/src/agent/event_envelope.rs",
    "echo-core/src/agent/mod.rs",
    "echo-core/src/error.rs",
    "echo-execution/src/tools.rs",
    "echo-orchestration/src/runtime/turn_driver.rs",
    "echo-orchestration/src/tasks/runtime_executor.rs",
    "echo-orchestration/src/tasks/runtime_service.rs",
    "echo-tools/src/code.rs",
    "src/acp/runtime.rs",
    "src/agent/mod.rs",
    "src/agent/react/lifecycle.rs",
    "src/agent/react/run/phases/finalize.rs",
    "src/agent/react/run/phases/mod.rs",
    "src/agent/react/run/phases/think.rs",
    "src/agent/react/run/phases/tools.rs",
    "src/agent/react/run/pipeline.rs",
    "src/agent/react/run/stream_channel.rs",
    "src/agent/react/run/stream_macros.rs",
    "src/agent/steer.rs",
    "src/agent/subagent/control.rs",
    "src/agent/subagent/executor.rs",
    "src/agent/subagent/team/mod.rs",
    "src/channels.rs",
    "src/eval/runner.rs",
    "src/headless.rs",
    "src/tools/builtin/agent_dispatch.rs",
    "tests/acp_agent_adapter.rs",
    "tests/agent_handle_turn_driver.rs",
    "tests/facade_smoke.rs",
    "src/acp/session.rs"
  ],
  "supports": [
    "behavior.task-subagent-execution",
    "rule.task-subagent-authority",
    "behavior.agent-turn-lifecycle",
    "rule.turn-terminal-authority"
  ],
  "limitations": [
    "Focused framework implementation checkpoint; six merge gates, independent-feature matrix, SDK/CLI consumer qualification and exact merged-main CI remain separate and incomplete.",
    "Original first 6472-byte before raw was overwritten by a later invocation; preserved command receipt, source and tool-session assertion remain, not a reconstructed full raw.",
    "Local raw is task-scoped, not bundled into the repository. Each reported run proves only its actual input cut; unavailable raw is not fresh passing evidence.",
    "The new real React Tool/Team task cases are after assertions, not represented as individually executed original-source counterexamples. Genuine Manager/pipeline/RunCode and resource counterexamples have separate retained before runs.",
    "Caller-provided opaque resource graphs require consumer ownership review. Internal retention has no manager/self projection cycle; arbitrary Any guards are not universally cycle-free."
  ]
}
---

# 原执行所有权与非终态观察

## 来源与范围

当前 framework 分支基于 46c0ee0104a3c2b68e818d7cf4008475a6d616f8（保留 main-only CI checkpoint），依赖来源为公开 00782。本对象绑定当前未提交源码摘要；正式规范为 ADR0088 与本仓库 B Design。SDK、accepted seed、wire operation 和 CLI 源码不在本切片修改范围。

复用唯一 RuntimeTaskService/RuntimeDagExecutor/SubagentExecutor、原 producer、原 admission 与 exact-claim CAS。首次 admitted 根取消/整体暂停仅启动 observer deadline；普通长执行无 grace timer，exact A 不启动根计时或打断 sibling。ExecutionNotSettled 保持非终态且保身份，fallible classifier、driver、input watch 和直接消费点不制造 Failed/Drained/Dropped。Unknown 资源保留在原 authority 中，不因 observer/service Drop 或缺本地登记而重执行。

## 原失败与受限历史

原 held-cleanup 反例退出101，显示 original_claim_retained=false、physical_drop_before_release=true 与 Cancelled 观察。最早完整6472-byte raw误被后续调用覆盖；原命令收据、源码与工具返回中的实际断言仍保存，不声称旧raw当前可复核。修后同case真实通过。

资源析构重入公开 API 反例实际101（2秒内无法重入），修后原case0；foreign setup 的 full run/execution 与唯一 invocation guard 修前Drop1的 before2实际101，修后保原guard及admission。初次不完整fixture、API迁移编译错误、旧永久pending fixture中断、错误过滤器选0均单独保留，不当作行为红或通过。

## 支持的结论

Canonical started/draining Tool 调用在 Manager configured timeout、React cancellation grace、batch timeout、首Complete与输出receiver关闭时保留原 future/stream、原permit和guards。信号限定Tool child scope；原read/write semaphore仍是唯一quota。首Complete业务字段保持到原EOF，late cleanup timeout/cancel不覆盖它；typedUnknown拒绝任何tool完成、callback、transcript或LLM continuation。普通非draining方法的原bounded策略不变。

真实 Manager producer写入effect文件并保持finally时，修前timeout/Complete/Unknown/receiver-close会提前返回或释放原permit；原before2/3/4与delivery-before的实际失败均保留。修后同强例0，先release并join原producer再断言，不留下红例后台任务。真实Tool→pipeline的unknown与closed-delivery也分别修前101、同case修后0。RunCode仅在shell feature实际选中1个原反例时得到行为红；初次0选择不算证明，修后同case0。

新增真实React→Tool→Team三例检查：顺序/并发跨原默认5秒grace，batch使用原20ms+250ms预算；原producer仍写effect、Task仍Running且完整原claim/中断事实/active owner保持。释放原producer后原终态和claim清理完成，重放旧claim被拒且关系/执行快照不变。测试中的关系revision原错误+1与ToolTimeout原错误TimedOut预期按既有00782合同纠正；失败保留。执行CAS不改关系revision，ToolError的既有terminal分类仍为Failed。

原永久pending intervention改为显式Notify结束：原10ms+250ms batch后collector仍pending，释放原前置callback后再结算。对已取消、未执行/未启动terminal callback、无结果且非typedUnknown的失败，复用原interruption projector记录实际Skipped/trace/audit一次。原单次call/result/error、redaction、成功peer和trajectory断言全部保留。

## 已执行验证与切点

最后相关源码：Manager全lib335/0/0；React run189/0/0；Subagent executor69/0/0；RunCode shell16/0/0。命令分别为`cargo test -p echo_execution --lib --locked`、根crate `--no-default-features --features subagent,acp --lib`下`agent::react::run::`与`agent::subagent::executor::tests`、以及`cargo test -p echo_tools --features shell --lib code::tests --locked`。各原日志完整、不截断、编译诊断零，每次581 Rust/Cargo输入前后相同；Manager335之后其源码未再修改。`cargo fmt --all`及`cargo fmt --all -- --check`均0、0bytes并且581输入无变化，diffcheck0，当前同ID Plan validate0。

此前各自切点Core426/Orchestration473/Team30/ACP10均0；无相关源变化时保其有限结果，不将不同切点或重复测试累加为unique覆盖。原Subagent65/66、React84和各编译/fixture失败仍为历史，不替代最后189/69切点。

## ACP Session close增量

87223c8163f6b30479fbd44eadd09736db50bd5d是已提交并推送的前一focused源码切点，不把其335/189/69/16集合冒充本次重跑。此后唯一新增生产文件src/acp/session.rs：close/close_all两个真实before各在原file backend held时把身份变Other、generation1变2、实际close调用2，2877B/exit101；无identity owner-loss真实before也在backend held时回Failed并重发，1402B/exit101。两轮编译诊断零，各581输入无漂移；释放并join原backend后才断言。

修复复用原receipt的私有NotSettled(Some原run/execution)/None知识，保同generation与Session/Agent。Some返回原typed，None用原Notify观察且不造identity或新owner task。close_all先join全部原close并保各自receipt，再返回首原typed；普通returnedError的原失败重试/成功清理/latelease语义不变。当前全原ACP Session模块18/0/0、2160B、零编译诊断、581guard0，原普通失败新generation与close_all retry等均实执行。当前fmtall/check各0、0B且581无变化，Plan官方validate0。独立增量复审绑定40source-doc冻结，PASS C0/I0/M0。

## 已知缺口

Raw receipts, failures/passes and input maps remain only in this checkout's
.echo-semantic/local/subagent-physical-settlement-b/; they are not source_refs or
committed raw archives. The 40 source/test/maintained-document files are frozen.
Final independent source review is PASS C0/I0/M0, recorded by the Finding's Audit.
Remaining acceptance is incomplete and the Finding stays open. Six merge gates,
applicable features, SDK/CLI consumer pins and exact main CI are not borrowed from
focused results. CLI opaque workspace ownership is separately validated by its
consumer; this Evidence does not claim combined qualification.
