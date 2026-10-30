# ADR 0035: Owned handles for the Tool registry

- Date: 2026-09-14
- Owners: `echo-execution/tools`

## Status

Accepted.

## Context

`ToolManager` stores `Box<dyn Tool>` directly in a `DashMap`. Its public
`get_tool` returns a `DashMap::Ref`, and both streaming and non-streaming
execution retain that reference through validation, permit acquisition,
retries, timeout handling, and the complete asynchronous Tool call.

`replace` and `unregister` are synchronous registry mutations. They can block
while an execution holds the shard reference. On a Tokio current-thread
runtime, calling either mutation from the executor thread while the Tool future
is suspended can prevent the same executor from resuming that future and
releasing the reference.

The Tokio shared-state guide warns that a synchronous guard held across
`.await` can deadlock even when its type is `Send`, and recommends limiting the
guard to a non-async scope. DashMap documents that mutations such as `insert`
may deadlock while a map reference is held. OpenAI Codex stores Tool runtimes
as `Arc` values and clones an owned runtime handle before async dispatch.

- <https://tokio.rs/tokio/tutorial/shared-state#holding-a-mutexguard-across-an-await>
- <https://docs.rs/dashmap/latest/dashmap/struct.DashMap.html#method.insert>
- <https://github.com/openai/codex/blob/16537b20a5ec0ea9aa079f4ad4b0e30e8a9efacf/codex-rs/core/src/tools/registry.rs#L459-L463>

## Options Considered

1. Require callers to move every mutation to `spawn_blocking`. This leaks the
   registry's lock implementation into every embedding application and leaves
   `get_tool` able to create the same hazard.
2. Keep the Box registry and add a second Arc execution registry. This creates
   two Tool generation authorities and makes publication ordering ambiguous.
3. Return a boxed proxy that forwards to an Arc-backed Tool. The proxy must
   duplicate every current and future `Tool` method, making risk, validation,
   streaming, schema, and sandbox behavior prone to wrapper drift.
4. Store one `Arc<dyn Tool>` per registry entry and return owned Arc handles
   from lookup, replacement, and removal.
5. Replace the registry with an actor protocol. This avoids synchronous locks
   but turns all builder and query operations into messages and is larger than
   the confirmed lifecycle defect.

## Decision

Choose option 4.

- `DashMap<String, Arc<dyn Tool>>` remains the only Tool catalog authority.
- Registration and builder APIs continue accepting `Box<dyn Tool>` and convert
  it once to an Arc at the registry boundary.
- `get_tool` clones an Arc while the DashMap reference is in a synchronous
  scope, then returns the owned handle. No registry guard crosses an `.await`.
- `replace` and `unregister` return `Option<Arc<dyn Tool>>`. A caller may inspect
  or retain the displaced generation without keeping the registry locked.
- The inherent `ReactAgent::replace_tool` and `ReactAgent::remove_tool` methods
  follow the owned Arc return contract. The object-safe `Agent::remove_tool`
  method continues returning `bool`.
- A lookup racing with mutation may receive either the complete old or complete
  new generation. An old generation already in flight may finish normally;
  removal is not redefined as cancellation.
- 工具owned Arc与结果缓存epoch在原registration_lock内同步取出；所有注册、批量注册、
  替换和移除沿同一把锁完成发布与cache失效。锁在任何validator、permit或执行await前释放。
  缓存读取在原cache锁内核对该epoch；原store回填fence继续阻止旧结果进入新代缓存。

## Framework And Application Boundary

Owned Tool generation lifetime is a reusable framework primitive. It does not
depend on EKO workspace, UI, permission, reload, or product policy. Embedding
applications can decide when to mutate a registry, but they do not maintain a
parallel Tool catalog or generation counter.

## Consequences

- Synchronous lookup and mutation hold DashMap guards only for bounded map
  operations. Tool execution no longer couples executor progress to a shard
  lock.
- Old Tool resources are dropped after the registry and all in-flight or
  externally retained Arc handles release them. Async cleanup still requires a
  separate explicit lifecycle contract; Arc Drop is not a settlement receipt.
- Rust callers that explicitly require `Box<dyn Tool>` or DashMap `Ref` return
  values must migrate to `Arc<dyn Tool>`. Callers using `is_some` or Tool methods
  through dereference remain structurally equivalent.
- The affected SDK identities are classified `host_or_rust_only`. Rust public
  inventory signatures and digests change, while TypeScript, Python, Java,
  extension wire schemas, and facade operation counts do not gain new APIs.
- No dependency, persistence format, protocol, permission authority, or
  application state is added.

## Compatibility And Rollback

This is an intentional Rust source compatibility change required to remove a
deadlocking public lifetime. Registration inputs and the object-safe Agent
surface remain compatible. The dynamic Tool example and SDK inventory document
the migration.

Rollback means reverting the registry and public-return commit together with
its generated inventory. Reintroducing a borrowed map reference across async
execution is not an acceptable partial rollback.

## Verification

A bounded red command first demonstrates that the old implementation cannot
complete current-thread replacement while an active Tool holds the registry
reference. The repaired tests perform replacement and removal synchronously
while the old Tool is suspended, then release and settle the old generation.
They also prove that the replacement is visible, removal makes subsequent
lookup fail, and the old generation cannot repopulate the current result cache.

Existing ToolManager registration, cache, streaming, validation, retry,
timeout, cancellation, and drain tests remain green. The dynamic Tool example,
Rust public inventory generator, SDK contract checks, independent feature
checks, Clippy, semantic snapshot/change evidence, and Issue reconciliation are
also required.

## 2026-10-30：许可与同一工具代次绑定

父Asset的独立源码审查发现：PermissionStage以工具A求值后，START callback可把同名
工具换成B；原ExecuteStage再次lookup B，仅重核readonly，可能将A的许可误用于B。
该问题在本地桌面也成立，修复防框架热替换误执行，不为GUI直接终端／MCP增加权限闸。

候选方案是每次异步重新审批、另建generation登记册，或复用原owned Arc。选择最后一项：
permission与Hook批准持有同一Arc，参数／streaming分类沿相同实例。原两个checked
draining入口显式接收expected Arc，原get_tool后、validation await／cache／effect前
同步比较同实例；变化则拒绝本次调用，不能自动给新实现沿用旧许可。已经选中A再
replace时A仍可正常结束，非checked调用仍是caller-owned执行原语。

这是两项公开Rust签名增量；没有第二工具catalog／executor／审批循环，没有新增
wire或ToolContext字段。原cache key、write失效和旧结果回填fence不变。
owned选择与cache epoch同步配对，cache读取另核对同一epoch，约束见下节。
SDK accepted与Rust inventory的实际影响由后继唯一完整producer列出，不自动新增
跨语言能力。验证要求保留原permission rewrite／protected path、Plan／readonly与
物理Join控制，并增加确定性START替换stream／non-stream反例和已选中A继续完成正控。

本段记录批准的修复方向及验收要求，不声明编译、运行或消费者已通过；各票须绑定
当时精确源码。回滚若恢复不同代许可缺口不可只留下metadata或测试通过声明。
## 2026-10-30：缓存结果与读取来源保持同代

来源绑定后，成功read结果中的FileRead会作为已授权owned工具的读取事实。仅防止旧结果
回填不足以保护该事实：A的validator挂起时B替换并填入新缓存，A恢复不能取B的effects；
反向也不能在B已发布、epoch尚未更新的窗口把旧A缓存记成B来源。

保留原catalog、registration_lock和cache epoch，选择private tuple getter在原注册锁内
同步取owned Arc与epoch；所有mutator使用同一注册锁。随后释放注册锁／DashMap引用，
再进入validator、cache、permit、retry与物理执行。原cache读取在与失效／回填相同的
临界区检查expected epoch，不一致则实际执行已经选中的原工具。已选中A仍允许完成，
不增加重审批、重试、generation counter、cache key或第二registry。单独提前epoch bump
会形成反向窗口，因此不选；仅在lookup比epoch无法覆盖发布窗口，也不选。

锁序为mutator的registration→短DashMap→cache；tuple getter是registration→短DashMap，
cache lookup／store与write失效只取cache，均不反向取得registration。公开get_tool仍返回
owned Arc，permission与实际Manager通过原expected Arc核对衔接；同步锁不跨await。

验收分层：原方向由实际pipeline、独立A/B provider、写入字节与两个stream模式断言；
反向由echo_execution自身cfg(test)的单Manager publication observer，真实replace发布后
暂停，调用原checked stream／nonstream，核B实际读取、完整ToolResult/FileRead与新B缓存。
跨crate的cfg(test)不传播，反向测试不宣称跑了完整pipeline；不为测试增加公开API、feature、
生产状态或循环dev-dependency。两个层级共同覆盖源码不变量，后续全量门禁与消费者另验。

本段不代替对应冻结源的实际RED／GREEN、独立复审或全量门禁回执。
