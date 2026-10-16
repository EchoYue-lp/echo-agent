---
{
  "schema_version": 1,
  "id": "evidence.subagent-physical-settlement-repair",
  "kind": "evidence",
  "observed_at": "875cc22f2e99c7ccd83c07e02c03fe221c5e3f6c",
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
    "src/acp/session.rs",
    "echo-agent-learning/tests/example_contracts/demo54_headless.rs",
    "echo-orchestration/src/scheduler/runner.rs",
    "echo-state/src/delivery.rs",
    "echo-orchestration/src/workflow/graph.rs",
    "echo-orchestration/src/workflow/checkpoint_store.rs",
    "echo-orchestration/src/workflow/node.rs",
    "echo-orchestration/src/workflow/mod.rs",
    "docs/en/17-graph-workflow.md",
    "docs/zh/17-graph-workflow.md",
    "echo-agent-learning/tests/documentation_contract.rs"
  ],
  "supports": [
    "behavior.task-subagent-execution",
    "rule.task-subagent-authority",
    "behavior.agent-turn-lifecycle",
    "rule.turn-terminal-authority",
    "rule.fact-projection-separation"
  ],
  "limitations": [
    "Final six plus17 gates actually pass Rust/Cargo inputs at public663cf644. This later document snapshot has unchanged581 code inputs and a fresh existing documentation runtime test; SDK/CLI consumers and exact merged-main CI remain pending.",
    "Original first 6472-byte before raw was overwritten by a later invocation; preserved command receipt, source and tool-session assertion remain, not a reconstructed full raw.",
    "Local raw is task-scoped, not bundled into the repository. Each reported run proves only its actual input cut; unavailable raw is not fresh passing evidence.",
    "The new real React Tool/Team task cases are after assertions, not represented as individually executed original-source counterexamples. Genuine Manager/pipeline/RunCode and resource counterexamples have separate retained before runs.",
    "Caller-provided opaque resource graphs require consumer ownership review. Internal retention has no manager/self projection cycle; arbitrary Any guards are not universally cycle-free.",
    "The current Scheduler 53 and unchanged Delivery 14 focused results do not replace final six gates/17 features, accepted Workflow A implementation, consumer qualification or exact merged-main CI; historical 23 gates belong to f0a6 only.",
    "Workflow focused checks cover native Memory/File and original Rust Agent/Function sources; they do not prove SDK DSL Function support, registered Llm/Store Host behavior, every remote Store durability contract, real power-loss survival or Windows/Unix barrier equivalence. Graph-specific foreign/late-Unknown cases were source-reviewed, not separately executed here.",
    "The first bilingual-guide preflight record failed due to an invalid candidate symbol, then a chained shell still applied the authorized guides; it is not retroactively passed. Corrective exact663 restore, valid mutable-PF check, same candidate replay and fresh document contract are independently reviewed. The original old-PF mutable snapshot was not retained."
  ]
}
---

# 原执行所有权与非终态观察

## 来源与范围

当前 framework 分支基于 46c0ee0104a3c2b68e818d7cf4008475a6d616f8（保留 main-only CI checkpoint），依赖来源为公开 00782。本对象按源码摘要绑定修复分支；正式规范为 ADR0088 与本仓库 B Design。SDK、accepted seed、wire operation 和 CLI 源码不在本切片修改范围。

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
committed raw archives. The earlier 40-file review and the current Headless/example
increment each retain their actual source cuts. Independent source review is
PASS C0/I0/M0; the final six local merge gates are now actual green as recorded
below. The independent-feature matrix is now actual green below. The known live-source
observation-to-terminal consumer gap and SDK/CLI/exact-main qualification remain
separate and incomplete. CLI opaque workspace ownership is separately
validated by its consumer; this Evidence does not claim combined qualification.

## 最终源码与Headless消费闭包

此前40文件ACP修复切点已提交并推送为ff0e698e69746a8b55cc2261d0f678094f4bca4e，原source:951cf5e91f3d0fb879bda5e1960e21750accb35acdc3246c14fb6a900b53225e归该历史，未重标到新执行。此后三处Clippy短路折叠经独立增量审为等价；原full测试暴露的两个admission Drop期待Cancelled夹具，现严格要求Unknown、无settled、拒重复，仅显式原settlement callback才发布Cancelled。Headless原never-EOF夹具改为首次真实waiter Pending握手、Notify原EOF；原2s、父子/兄弟隔离、释放前close0与释放后close1保持。

Headless便利API另有真实生产反例：原React+LLM Tool调用已启动file effect producer并报告typedUnknown，便利函数返回时原唯一资源guard Drop1，producer仍held；release同join真closed后才触发断言，1183B/exit101，非编译错。修复只经返回HeadlessResult私有owner投影保同HeadlessRunHandle；state.publish只存owner-free观察，wait仅外向附原handle。原5公开字段签名与格式不变，6个外crate构造式逐5表达式一致迁移到new。修后Headless全11/0/0、1322B、581不漂移、零编译诊断；clone/getter同state、内部ownerNone、原effect/permit held与最后parent释放的Weak/guard/permit强assert通过。随后仅加强同2s析构尾观察为weak0/drop1/cap1，最终完整测试实际包含该当前case。可能取消等待且需后续保原owner者使用start_headless并保handle；不承诺销毁所有parent后永存或自动恢复。

## 完整本地门禁

最终源码上六个原命令均实际0且每次581 Rust/Cargo/config输入前后一致：fmtall/check各0B，全target全feature Clippy1454B、production严格Clippy1454B、完整workspace/alltargets/allfeatures测试359273B、featureless workspace lib check3443B。两Clippy与check/compile诊断零、日志不截断；profile统一JOBS1、DEV/TEST DEBUG0与STRIP=symbols、INCREMENTAL0，target限定本任务。完整测试86个Cargo主harness合3580pass/0fail/3ignored；echo_state两段嵌套child各1不重复加总。三忽略为既有live LSP、scholarly providers与Zotero凭据opt-in，未冒充执行。原预算中断/monitor失效/启动前空间拒绝完整保本地，均不是行为绿或已执行测试计数。

17个独立feature原命令均实际0、不截断且零编译诊断：acp/a2a/mcp/lsp/sqlite/telemetry/topology/subagent/web/media/data/statistics/channels/git/database/rag/chart。每次581输入前后相同、JOBS1同debug0/strip symbols/inc0；这23项工程结果只属于本源码切点，不证明未覆盖的语义义务已自动关闭。Rust源码/正式规范在该序列中冻结；本对象、Audit和baseline的后续更新仅为语义metadata，不改变这581输入。原件为任务本地full-*-strip3.{log,-run.json,-before.json,-after.json,-source-guard.json,-profile.json}及对应matrix原件，仅当前工作区可用，不将哈希当公开原件。当前Source摘要在完整六门禁后、feature矩阵期间观测；不是伪造全批启动前观测时间。

## 明确未闭合的live-source后继

实际live source可先报告ExecutionNotSettled并保原producer，随后给出同owner/generation的合法terminal和physical EOF。当前TurnDriver在观察分支停止正常envelope消费，raw drain不保留后到terminal；SubagentExecutor虽继续读，其saved physical_uncertainty在EOF后仍无条件上送。因此有效后到结算可能被旧观察知识覆盖。此源路径已识别，但本checkpoint尚未为其新增真实red/green或实施修复，不能把上述23绿冒充这条义务已闭。无source或无合法terminal的EOF仍需保持Unknown；后继必须保首真实业务cause、exact claim、input/owner/permit与wrong-generation拒绝。宽Finding保持open，Plan保持ready，不交付main。

## Live-source局部闭合与新的切点

上面的23门禁及“尚未新增反例”描述属于已公开开发checkpoint f0a6cd1c13706c20955209b698c1f891c09d78b3，不代表本次新源码的完整门禁。新Driver真实同源counter在原producer held、后到FinalAnswer与真实EOF均已发生后仍返回旧typed观察，2903B/exit101；执行输入与原源码副本保任务本地，非编译红。

当前复用原Core stream wrappers与normalizer：原raw source上的观察只可由同绑定run/execution的后到显式terminal及实际EOF消除，foreign/wrong-attempt、没有后terminal的EOF、terminal后再到观察、normalizer合成错误、同字面身份的sink观察都不冒物理证明。取消只固定首cause；默认wrapper保同source，只有实际闭合且观察已被后到原terminal确认时才发唯一取消terminal。Driver继续原envelope消费，不重放既有事件，原input watch在真结算时清观察。Subagent从同源projection得到真实结果后沿原controller/CAS结算，不重启执行。

同Driver counter修后实际0。最终相关集合Core全426/0/0（33599B）、Orchestration全477/0/0（42020B）、Subagent executor70/0/0（7486B）；真实Team normal/cancel两路径保原claim/active/guard与file effect至原EOF，随后同claim一次结算为Completed或Cancelled，claim清理、activefalse、执行计数1。默认包装器取消fixture改成真实poll接纳→EOF前pending→release→唯一Cancel，原2s/physical assertions保持；原两早Cancel断言及新未接纳取消的调度fixture首次34pass/3fail原件6618B/101保留，未放宽生产优先级。

每次581 Rust/Cargo输入前后同、日志无截断且编译诊断零。其后两个matchguard及tail let-chain只等价短路折叠，独立逐copy复核通过；三crate no-default/subagent,acp lib+tests Clippy原101两lint保留，修后同命令0/648B/零warning，fmtcheck0/0B。独立有限实现审查PASS C0/I0/M0，绑定七文件freeze2；后续Plan仅登记已确认Scheduler/Workflow必要后继，不改变Rust或以上测试切点。当前源码摘要在这些focused结果后观测，不重写执行时间。全部raw只在任务本地，不新增提交归档。

该切片只闭合同原live source的结算消费；Scheduler callback观察终态化/owner-loss重放，以及Workflow resume重排/heartbeat失败/TTL恢复仍须各自真实counter和窄设计。普通错误既有重试合同不因此全部改变。宽Finding仍open、Plan ready；旧f0a23不充当当前代码的六门禁/17feature、SDK/CLI或精确main CI资格。

## Scheduler原occurrence所有权增量

前面live-source结果已由公开ffff6101be6adc8f06351987e8c4166fd3bd98bd保存；其关于Scheduler未实施的句子是当时状态。当前Scheduler复用唯一OccurrenceAuthority、原DeliveryLedger/journal、原callback与claim，不增加scheduler/store/公共TaskStatus。新scheduled/manual payload以版本化私有保护合同持久保存，旧decoder拒绝该新wrapper。原EffectStarted新增可选replacement_payload，精claim/Claimed校验后与effect admission同event安装并按原envelope重算retained_bytes；None保持旧JSON，Some由旧deny-unknown decoder拒绝。旧历史可按原合同首次replay，但本次新physical attempt在callback前也必须同claim升级保护，不能借legacy标签重投已知未结算effect。

原callback factory与future均在同一个owned producer内执行，borrowed waiter取消/shutdown不drop它。typed观察或owner join loss不更新last_run、不提交Failed/Completed，也不把protected occurrence重排；其它独立occurrence继续推进。正常已结束success/error仍沿原结果和terminal策略。SchedulerHandle强持同Runner，join/shutdown改为借用&mut，原JoinHandle在Pending时不take，完成后缓存结果；重复观察不执行第二callback。只有调用方保留的原parent承担后续资源可达性，销毁最后parent不等物理停止，也不承诺无receipt自动恢复。

真实before包括：typed callback在后台file producer held时被记Failed/ERROR（1919B/101）；borrowed drop与shutdown提前丢原callback（2265B/101、2fail）；唯一Handle出口丢原Fn资源/permit（1439B/101）；legacy本次Unknown被重claim（1814B/101）。各例均先release并join同原backend后断言，无后台残留。修后同例通过，另核factory/poll panic、scheduled/manual两准入、真实旧字节恢复、独立journal路径、重复join与借用取消。新admitted Some不能被后到cancel或真实definition poison的准入gate提前短路；两实际before分别1576B/101与1780B/101，修后各同case0。poison在真实physical关闭后的旧projection错误仍保留，不清poison或强改成功。

最终Scheduler模块53/0/0（5470B）、原Delivery模块14/0/0（2874B，相关源码未再变）、fmtcheck0/0B、两crate lib+tests Clippy0/226B。最终四命令各581 Rust/Cargo输入前后同、零warning且无截断；不把重叠counter叠为unique。原主after编译错误、错误过滤0case、legacy fixture首次failure及Plan早收据覆盖限制保持任务本地事实，不当作有效红绿。独立实现复审PASS C0/I0/M0，绑定六路径freeze2 5836dad5e3dba290d0edefef8d4cf635dcf4acb1f8d54d0cbe6793e053cecf28。

文档、ADR与Rust调用迁移说明随源维护；当前无需要改变的示例值或website产品展示。最终workspace门禁将编译示例；本focused不借未执行的demo/consumer资格。Workflow A已由用户单独接受，绑定原SDK proposal SHA256 23c5a59ae405ecd71c3e79b6d945e77e8b7927e0e1f0176ad91047147341a09f，尚未由本Scheduler增量实施或验证。Finding保持open，Plan ready，最终全6+17、SDK/CLI消费及精确main CI仍待后续最终代码。

## Workflow A原claim与source的局部闭合

前述Scheduler切片已公开3dedb9a6834cd28a3718d9852023722a6d24f4dc，其“Workflow尚未实施”是历史状态。用户接受原proposal23c5的A后，现复用原CheckpointStore/Graph/Node/原目录OS锁：同claim positive protect先于Node effect；默认false的绑定Store在可执行resume/branch的claim/fork前拒绝，Deferred/Rejected/fresh入口保持区别。Memory保护只承诺同实例，File版本化私有记录保原attempt且禁止expiry/CRUD隐式解除。普通已知物理结束错误仍requeue，source-less或Store侧观察保typed身份与原claim。

原3ded Graph/Node同真实file backend的Memory/File重claim两例修前2748B/101，borrowed caller/stream Drop丢原Node guard两例1808B/101；均释放并join原backend再断言。修复只共享原不可变Node/Edge/Store能力，每次调用由一个owned原loop继续驱动原source，observer Drop仅signal/detach。取消、Node timeout、fanout panic/sibling与heartbeat失败均保同原future到真正EOF；panic分支自身已展开的Rust局部资源不伪称仍可保留。原Agent source身份独立绑定；matching观察后真实terminal+EOF可原claim一次结算，取消首cause保留，无terminalEOF仍未知。Core默认structured plain-user chat复用原raw adapter，不用适配器synthetic EOF冒原source关闭。

marker混旧/损坏两例真实2495B/101后同例0；早编译错和未真正准入backend的chat fixture失败单独保留，不当作物理红。valid chat before1249B/101和fanout1574B/101修后同例0；身份loadA/claimB、first-buffered-yield Drop、heartbeat三个定向inverse真实101后严格恢复原correct source，最终Graph模块实际覆盖其after，不冒公开3ded原before。Store renew/requeue typed观察两例2299B/101后同例0，Node EOF不清不同Store权威；普通aux requeue消息记录diagnostic而不替换首业务cause。

File实际protect bytes在持久测试中对原旧wrapper/bare两个decoder均拒绝。冻结3ded旧File模块、未变共享types/utils及精依赖闭包的本地probe完成8原automatic methods拒绝/bytes与mode不变、2真legacy控制及4真实双进程原同fd锁交错；每次实际WouldBlock后才放行，不借最初sleep-only竞争推定。它不是整个旧F二进制或任意旧writer保证，旧delete/clear仍不安全，必须禁止降级接管。原atomic_write seam注入可见写后Err证明Node0及同attempt重试重新写barrier，不当作实际内核掉电试验；observer Drop时原blocking writer仍持同OS锁。全部probe/源码副本/raw只local，不入Git。

最终focused为Core全427、Orchestration全508，均0失败/0忽略；Graph模块包含45例（不与全lib叠加），Store16包含于其中。两crate all-target Clippy与fmtcheck实际0、零warning/无截断，各581 Rust/Cargo/config输入前后一致。独立有界Graph/Core实施审与NativeStore专审均C0/I0/M0；只支持此开发checkpoint。Graph-specific foreign/lateUnknown仅本次source审，不借Core/Driver旧负例当新Graph运行。文档/ADR/迁移同步，examples与website无需改产品值；最终workspace门禁仍须实际编译示例。宽Finding open、Plan ready，最终6+17、SDK三语Host/CLI消费及精确main CI尚待最终代码，不借f0a历史23资格。

## 最终框架门禁与指南运行输入桥

上述Workflow局部由公开663cf644ab6ad7b039366529bdd0e5837d922cf5/treee3385a74f71b98985f9e7e9ae2900e77949d7c1f保存。本次在该精确源码执行完整六门禁与17个独立feature：六项均actual0，原bytes依次0/0/19617/3056/377315/3443；完整测试88个raw结果块3622/0/3，扣echo_state两嵌套子harness各1后为86主块、3620pass/0fail/3既有opt-in忽略。三忽略仍是live LSP/scholarly/Zotero，未冒执行；criterion Success属于原all-target链路，不另加用例计数。17个acp至chart独立检查全部actual0、无截断、零编译诊断。每项581 Rust/Cargo/config输入前后精同，JOBS1、DEV/TEST DEBUG0、STRIP=symbols、INCREMENTAL0，原9GiB/free5GiB资源守卫从未降低；预警/原历史中断均不当作绿。

23运行期间源码及原规范冻结在663。随后只同步中英文17号指南checkpoint章节，删除不存在的run_with_checkpoints/resume_from_checkpoint/latest示例及protected claim可TTL重投的旧承诺，复用真实Graph API和已执行demo39基础Graph/state链接；不宣demo39演示checkpoint。保护能力在claim/prefork之前拒、真闭合才ack/requeue、不同Store观察、Memory实例/File原锁与旧delete/clear限制均与已接受A一致。指南之后的新Source摘要包含这些文档，不能冒它是23最初运行摘要。其581运行码与663全部相同，唯一实际runtime读取指南的既有documentation_contract使用full05原all-feature二进制与原test名新跑1/0/0（193B），binary/doc/testSHA、before/after均留任务本地，不新Rust测试/feature组合。

首次guide预检候选符号错误导致record失败，链式shell仍apply了已授权2doc；旧PF完整mutable快照未保，不倒追认首次。纠正链真实保存当前有效宽PF的两路径allow/mutable证据，check0后仅2doc恢复精663（其余tracked不动、diff空），再次前置check0后重放同候选，再用同binary同case得到contract2的193B0，并做finalcheck0。原failure与纠正时序均保，后续步骤显式失败即停。独立最终有界审核内容、实际重放、23原件与581桥C0/I0/M0。

这些结果支持框架分支交付准备；不证明SDK注册Store/三语Host和CLI已消费最终main，也不代替精确merged-main CI。Finding open、Plan ready保持；最终canonical快照/change与staged artifact另按实际候选验证。网站不在本仓直接改，正式指南由对应website lane同步；框架示例已在workspace all-target测试链真实覆盖。


历史快照固定：后继ZIP依赖修复开始时，将原source:736ac2观测固定到875cc22f；canonical对该Git树实算源码digest仍精等736ac2。此为同一旧源码/文档的可恢复定位，不新增B测试、复审或mainCI通过声明。
