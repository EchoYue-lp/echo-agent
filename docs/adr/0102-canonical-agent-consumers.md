# ADR 0102：现有消费者采用唯一原生 Agent 输出

## 状态

已按Root批准范围采用到当前分支，正在同一精确target验证；未授完整Goal或下游资格。2026-10-28用户最新Goal明确开发期
不承担旧版本兼容，旧格式恢复成功与旧wrapper透传验收义务已取消，不标matched/pass。
原EKO整批租约已由Root关闭；本轮仅在Root精确v2窗口沿原source安全切点整合。

## 背景

当前Framework的Subagent executor和Workflow Agent node仍只从Token/Think旧投影消费
增量，原生ReAct已只产AssistantMessage/ExecutionStatus，造成现有能力的消费缺口。
Subagent还按Message.content是否为Text选择旧字符串入口，丢失Text消息携带的role/name
等typed元数据。这两项是当前功能漏洞，不能因取消旧兼容而省略。

## 候选方案

1. 在provider重新生成旧Token/Think，或按正文猜phase：继续双轨，且会把普通正文误当推理。
2. 在Subagent和Workflow各实现一套执行/终态适配器：复制owner与取消权威，不采用。
3. 保留原source/执行循环，消费者使用同一纯输出归并器：采用。归并只整理当前原生事实，
不创建执行、Store、恢复或终态权威，不从缺字段或旧Final-only流合成Started/Settled。

## 决策与边界

新增Core公共Rust小型投影helper AgentOutputProjection/AssistantTextDelta，供独立crate
的现有两消费者共同调用，避免复制request/block去重与终态校验语义；不是新wire/schema
或新的业务状态机。新API须纳入SDK公开Rust inventory独立核对，不预报SDK接受。

每个实例绑定一条原生execution，按request_id/block_id/sequence保全观察与精确去重；
同一坐标不同payload明确失败，不按相同文本跨请求去重。Reasoning与普通文字独立，
signed/opaque只取原生可呈现thinking/summary，不将signature/redacted/data输出到正文。
原native payload仍由原Message/provider回放保存，本helper不改写它。

Subagent始终使用Message+invocation方法，前置history保在invocation；Workflow沿原
String输入合同构造Message::user，不据此声称支持其原接口没有的多模态输入。两者均
消费AssistantMessage和原运行时ExecutionStatus，旧Token/Think变体拒绝，不补成功。
原source未知观察只有匹配原execution的真实runtime terminal才能解除；Final确认和
block.complete均不得解除。原流读到EOF，后续cleanup错误仍是失败，取消和资源束继续
由原owner保有；输出投影不代替物理结算。

合法的新native Cancelled/Failed在核验身份、序号并清理匹配unknown后，以纯只读
terminal_failure投影进入原消费者首因槽，避免后续cleanup错误覆盖原取消类型或失败原因。
重复/foreign/合成终态没有这项资格；foreign unknown仍优先。Settled无失败投影，
其后唯一cleanup错误仍为失败，所有分支保持原source读到物理EOF。

运行时FinalAnswer事件、on_final_answer回调及最终验证/收尾能力按用户原方案保留；
具名退役的是FinalAnswerTool，不因同名把当前runtime通知当旧兼容删除。无需更改
事件shape，也不另造final owner。现producer在Guard转换时不发布原raw块，安全正文
由这条运行时通知完整承载；consumer在本请求批准块已完整呈现相同最终正文时不双拼，
否则只补真实运行时通知的安全正文。读取原source到EOF后，使用最后真实runtime terminal已有request_id核对批准块；没有该坐标不猜最近Unknown或跨request去重最终通知。
最终结果与结算分别记录：FinalAnswer确认不能解除unknown或代替原runtime terminal/EOF。

## 兼容退役与保留能力

旧text-only Agent默认wrapper、adapter_generated身份合成、缺终态推断成功、旧think/
final断点归一化、旧schema数组/缺字段猜测与旧配置alias验收按用户新决定退役。具体
删除候选/调用方/替代路径见同批retirement-inventory.json；不得新增skip保持旧测试。
当前writer仍写的格式须同安全切点收敛后再删旧read支，不迁移用户原数据。

String便利、chat/history与execute fresh语义、多Store、非流式模型传输、当前cold/Fork/
恢复/幂等/CAS/副作用不重放，以及第三方原生reasoning签名/加密回放继续保留。

## 验证

验收覆盖纯归并器、原executor typed/full EOF与attachment失败drain、Workflow execute/chat、
tail error/foreign terminal、无重复输出和EOF前不提交状态。2026-10-28已按精确target取得
两消费者当前缺口的tests-first RED→GREEN；独立审查补出的native失败后cleanup首因组合
亦先在Subagent与真实Graph.run断言失败，再按同命令验证通过。各原始自然退出与源码
指纹分别保留，不把编译/fixture失败当作目标RED，也不把早期带warning的自然0当最终收口。

当前focused回归与四crate lint不代表完整workspace/feature矩阵、SDK Host、EKO或现场验收。
旧3959不借作当前PASS。原既有旧协议fixture须按inventory退役或改为当前native fixture，
不以skip维持旧协议成功义务。

## 依据

原canonical合同见ADR0101及echo-core/src/agent/mod.rs的block/status定义。provider
坐标分离与完成事件不等于业务执行结算的参考来自官方OpenAI Responses事件合同
（https://github.com/openai/openai-python/tree/main/src/openai/types/responses）及既有
Anthropic原生message协议。这里只复用已经接入的字段，不从第三方事件推断本地owner。


## 当前非流式能力与产品Fork证据

Framework String便利execute/chat和execute_typed仍驱动同一个run_stream_entry/Managed
source，读到原EOF且观察到Settled才返回；model_streaming(false)只选择LlmClient::chat，
完整响应再适配同canonical块/工具/usage及同ReAct。这些是当前能力，不能按方法名退役。

当前Framework耐久Message保存的是含原execution/request/sequence及approved投影的Candidate；
真正runtime Settled在持久化/校验/收尾后公开。不能把耐久Candidate升级为成功来满足Fork，
也不能由App用返回String或TaskRunCompleted补造终态。GUI主路径原AgentTurnDriver→Sink→
ChatEventLog保同一产品conversation/generation的实际最终通知及Settled/EOF；Fork需要该
真实产品发布证据。Cron RunJournal与任意direct Framework Store并不自动属于该GUI
generation，不能互相冒替。取消无native时空events/Ok旁路的旧兼容义务时，保留已具有
上述实际发布证据的合法nonstream GUI回复；没有证据明确不授Fork，不删String便利能力。


## 本轮真实输入与验证状态

本轮先采用共享纯projection基础与真实consumer测试，旧producer保持原样。Core13项
已实际0，Subagent与Workflow分别精确list1后实际101，明确Text降级/旧文本入口，
不是零匹配、compile或fixture超时假RED。初次ToolAccess无Serde编译错误与早EOF
被夹具timeout遮蔽的原件保留，只修原typed断言和原JoinHandle提前结果观察，不加
公共Serde、不加时。原reader/lsof末端核空、2803源tar逐member保全后才应用生产hunk。
两消费者同命令GREEN与后继首因组合已实际自然0；原ReAct/HTTP/Stop保留面、最终Review
与lint按各自精确源码票分别记录，不以源码审查替代运行，也不预授完整workspace资格。
原metadata增量task99e4只归e0HEAD六份材料，不关闭原迁移45/#10/旧402。
SDK Rustinventory/newGitPin、native/MainCI另验；原runtimeFinal通知/回调始终保留。
