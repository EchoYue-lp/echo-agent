# Telemetry Host 组合修复候选

状态：技术修复设计；06ec0e0a 版本经独立复审零问题后获主代理批准实施，随后单 fmt/ConsoleFilter 技术形状也获明确批准。本文不接受新的跨语言承诺，工程与交付门禁尚待完成。

## 已复现问题

三个语言各用正常 Host `bcb0389…`、公开 `Session.invoke` 调用 `telemetry.status/init`。初始化前为 false，init 在两秒测试观测窗口内没有完成，正向成功断言真实失败；没有收到 FrameworkError，也没有查询到初始化后的 status。两秒只限制测试观察，不修改生产 deadline。

自有 Host PID 58879 的一秒 OS sample 指向 `init_telemetry → SdkMeterProvider::drop → shutdown → PeriodicReader::shutdown → futures_executor::block_on`。源码同时证明 Host 已安装全局 stderr subscriber，而框架随后第二次 `try_init` 必然冲突。实际阻塞在该失败路径释放 meter provider 的同步等待，不在 exporter 构建入口。原始日志、三个语言输出、父子进程关系和 sample 见 receipt。

## 保持的行为与分层

框架保留唯一 METRICS、TELEMETRY_RUNTIME、初始化互斥、OTLP providers/exporters、采样和 shutdown 权威。SDK adapter 只转换参数、组合 stderr/ACP 和选择正确的异步执行上下文，不维护第二 telemetry 状态机。

保留现有公开 init/shutdown 签名及一次性政策：成功 init 后再次 init 返回错误；shutdown 刷新现有 providers；shutdown 后 OnceLock 不清空，status 仍表示“曾成功初始化”，重复 shutdown 返回原 provider 错误，不暗改成可重新初始化或幂等成功。三个现有 wire 操作及签名保持；必要 Rust 组合 helper 仅 Rust/Host 使用，不能自动接受完整新 identity 为三语言义务。

## 方案比较与推荐

1. 忽略 `try_init` 失败、只安装 metrics、移除 Host stderr 或把 console 写 stdout：拒绝。它们分别伪报成功、丢失已接受 tracing、丢失现有诊断或破坏 ACP。
2. 用 `reload::Layer` 包整个 OTel layer：拒绝。锁定的 tracing-subscriber 0.3.23 官方文档与源码明确其 downcast 限制；tracing-opentelemetry 0.28 依赖实际 layer 的 downcast 取得 span 上下文。不能用一个 reload 成功结果代替 parent/context 正确性。
3. 推荐稳定安装 OTel layer，并用薄 tracer 委托同一框架 runtime；独立解决同步生命周期调用的执行上下文。只使用现有 tracing/OpenTelemetry 标准 Layer、Filter、Tracer、PreSampledTracer 原语，不实现采样、trace ID 生成、批处理或另一 exporter 调度器。

## 稳定 layer 与初始化

框架提供 Rust composition helper 返回可与 Registry 组合的稳定 OTel layer，以及只读的 ConsoleFilter helper。Host 启动时一次安装 OTel layer 与原单一 stderr fmt layer；该 fmt filter 将原诊断规则与 console 规则做标准 OR 组合，既保留初始化前诊断，也避免启用 console 后重复格式化。后续 init 不重新安装 subscriber，不创建 SDK wire DTO。

OTel layer 保持地址和类型稳定。其 tracer 的 `Tracer::build_with_context` 与 `PreSampledTracer` 三项方法全委托 `TELEMETRY_RUNTIME` 内原 SdkTracer；缺少 runtime 时使用标准 NoopTracer/BoxedTracer 安全返回，不 panic、不构造自制采样或 ID。这样仍由官方 OpenTelemetryLayer 保存 OtelData、上下文和 parent 关系。

使用标准动态 per-layer filter，在 runtime 尚未成功发布时不让 OTel layer 接收 span；其 callsite interest 必须保持动态，不能把初始化前 disabled 缓存为永久关闭。无需 reload 整个 OTel layer；若实际实现改用 reload，只能 reload Filter 并证明不会破坏 downcast。原有 Host stderr/RUST_LOG filter 独立保留，不用 telemetry 开关禁用它。

组合 helper 可附一个无运行状态的 layer marker，通过正常 Dispatcher downcast 识别兼容组合已安装；它只证明 subscriber composition，不拥有 telemetry 生命周期。没有兼容组合的 standalone 调用仍按现有 init 行为安装框架 subscriber；遇到已经存在的非兼容外部 subscriber，必须明确返回错误，不能装成功。检查冲突尽量在构建 providers 前完成。组合是否成功必须依据实际 subscriber 安装结果，不能构造 helper 就宣称已安装。

init 在原初始化锁内构建实际 exporters/providers，最后发布原 METRICS/RUNTIME。构建或安装失败不发布 initialized，不更换现有 subscriber，不遗失尚待释放的 providers；清理在允许同步等待的执行上下文完成。失败后没有成功 runtime 时，仍允许按原未初始化语义再试。

## 最小 Rust 类型关系（候选，尚未编译）

锁定库中 `BoxedSpan::new` 是 crate-private，不能直接调用。可用公开 `BoxedTracer::new(Box<dyn ObjectSafeTracer + Send + Sync>)`：其标准 `Tracer` 实现的关联 Span 就是公开 `BoxedSpan`，且库已为 SdkTracer 与 NoopTracer 提供 ObjectSafeTracer blanket 实现。因此只通过 BoxedTracer 获得同一 Span 类型，不自写 Span 包装或 SDK 采样逻辑。

```rust
// 新增仅框架 Rust/Host composition helper；不新增语言 API。
pub fn telemetry_layer<S>()
    -> Box<dyn tracing_subscriber::Layer<S> + Send + Sync + 'static>
where
    S: tracing::Subscriber
        + for<'a> tracing_subscriber::registry::LookupSpan<'a>
        + Send + Sync + 'static;

pub fn telemetry_console_filter<S>()
    -> impl tracing_subscriber::layer::Filter<S> + Send + Sync
where
    S: tracing::Subscriber
        + for<'a> tracing_subscriber::registry::LookupSpan<'a>
        + Send + Sync + 'static;

// 以下均 private；没有单独 exporter/provider/global Runtime。
struct RuntimeTracer;
struct TelemetryLayerMarker;

impl opentelemetry::trace::Tracer for RuntimeTracer {
    type Span = opentelemetry::global::BoxedSpan;
    fn build_with_context(
        &self,
        builder: opentelemetry::trace::SpanBuilder,
        parent: &opentelemetry::Context,
    ) -> Self::Span {
        // 导入 Tracer / TracerProvider trait；分支都经标准 BoxedTracer。
        match TELEMETRY_RUNTIME.get() {
            Some(runtime) => opentelemetry::global::BoxedTracer::new(
                Box::new(runtime.tracer_provider.tracer("echo-agent"))
            ).build_with_context(builder, parent),
            None => opentelemetry::global::BoxedTracer::new(
                Box::new(opentelemetry::trace::noop::NoopTracer::new())
            ).build_with_context(builder, parent),
        }
    }
}
```

`RuntimeTracer` 的 PreSampledTracer 实现分别调用同一原 SdkTracer 的 `sampled_context/new_trace_id/new_span_id`；无 runtime 分支分别调用标准 NoopTracer 对应实现。稳定 layer 的 OTel filter 在 runtime 发布前关闭，避免 Noop 的 invalid ID 进入初始化后的采样 span。上面片段仅固定关联类型与公开构造路径，不是完整实现或编译通过声明。

组合内部使用标准 Layer::and_then/boxed 串接原 OpenTelemetryLayer 与无状态 marker。格式化由调用方的唯一 fmt layer 拥有，ConsoleFilter 只观察原 runtime 配置。marker 的 `impl<S: Subscriber> Layer<S>` 使用库默认 downcast，不手写 unsafe/downcast。只有实际安装后的 `Dispatcher::downcast_ref::<TelemetryLayerMarker>()` 成功才认定兼容组合存在；仅调用 helper 构造尚未安装时必须是 false。Box/Filtered/标准 Layered 必须在真实测试中保留该 downcast 与 OTel `WithContext` downcast。

Host 明确在 Registry 上先组合 telemetry layer，再附唯一 stderr fmt layer；其 filter 是原 Host EnvFilter OR framework ConsoleFilter，**原 Host EnvFilter 不应用于整个 subscriber**。不能把 Host 默认 `error` filter 放在全 subscriber 外层，否则会把 OTel 的 info span 和正常运行 trace 一并过滤。OTel layer 保留框架原 `RUST_LOG`/默认 info 过滤，并与 runtime 已发布的标准动态 filter 组合；console fmt 同样按原 runtime 配置开关，不遮蔽既有 Host 诊断。

## 边界 span 与 console

初始化前创建且跨越 init 的 span 不得在关闭时突然创建新的 OTel 身份或重新采样；它在 OTel filter 尚未启用时不被该 layer 采集。初始化后创建的 span 必须使用原 SdkTracer，保留有效 parent、trace ID、sampling 和 `OpenTelemetrySpanExt`。初始化前的外部有效 OTel parent 若显式传入初始化后的 span，仍按标准库传播。不能把初始化前日志“补发”描述为现有合同。

`enable_console` 控制框架附加 console layer，而不是 Host 原 stderr 诊断。standalone 保留原 console 默认目的地；Host 的唯一 fmt layer 明确提供 stderr writer，所有 console 模式都不得写 ACP stdout。框架 Runtime 可保留原配置所需的 console 开关，属于同一既有 runtime，不增加 SDK 全局 owner。唯一 fmt layer 的原诊断规则与 runtime console 规则取 OR，不能重复格式化，也不以关闭原诊断解决。

shutdown 后保留原一次性 initialized 事实，不替换 tracer provider 或允许重新绑定服务。已经结束的 span 与 metrics 在成功 shutdown 时按原 SDK 行为刷新；shutdown 后创建的记录不承诺导出。跨 shutdown 的活跃 span只遵循锁定 OpenTelemetry provider 的既有边界，不新增“任意活跃 span 必导出”保证。

## Host 调度修复

init/shutdown 保持框架同步 API，但 SDK 的异步 telemetry adapter 应将阻塞生命周期工作放入 `tokio::task::spawn_blocking`，等待真实结果后返回。exporter 批任务仍使用当前 Tokio runtime，不能创建第二 runtime 或把同步等待留在 ACP dispatcher 内。status 仍读取原 framework Metrics。

阻塞生命周期 owner 复用 `CoreProfileState::begin_settlement/finish_settlement/wait_settlements`，不创建 telemetry job registry。现 `SdkCoreProfile::wait_for_settlements` 已在关闭链中调用该 drain；原 Run driver/持久化 collector 也使用同一 counter/Notify，telemetry 仅增加它所统计的一个实际未完成工作，不借用 Run ID 或伪造 Run receipt。相关候选路径是 `core_profile/state.rs:378–425`、`core_profile/mod.rs:902–923`、`handler.rs:1142–1172`。

具体所有权与关闭顺序如下：

1. adapter 同步调用 begin_settlement 预留实际工作计数，随后重新检查原 connection `services.ensure_admission`；检查失败立即成对 finish，不提交任务。计数在检查前取得，且检查到提交之间不 await：若 close 先关准入，操作拒绝；若操作已预留，close 的现 drain 必须看到它。保持既有 facade 第一层准入检查，第二检查只闭合并发窗口。
2. 以当前 Tokio handle 提交一个 spawn_blocking，整个 init 或 shutdown 及其失败 rollback 在该原任务内执行。紧接建立一个独立的 Tokio collector，唯一持有该 JoinHandle、原 Arc<CoreProfileState> 与一次结果通道；不能把 JoinHandle 留给 RPC waiter。collector 必须由当前运行态建立，不能受 request cancellation/transport observer abort 取消。它不运行第二个业务调度器，也没有自己的 telemetry 状态。
3. collector 等待真实 blocking job 返回，再完成现 finish_settlement，最后将原 typed operation result 发给可选 waiter。waiter drop、RPC取消或 EOF 只失去结果接收，不能改变计数、abort blocking job 或伪报 job 完成。init 的显式错误仍原样返回；一次真实失败且 rollback 已结束表示工作已结算，不应把普通配置/重复初始化错误永久注入 Host 关闭失败。JoinError 则保留实际 panic/cancel cause，交现 close/error 通道处理，不填成功。
4. Host close 仍由原 connection close owner 先关新准入，再调用既有 `SdkCoreProfile::wait_for_settlements` drain。超时返回原 incomplete close，原 collector、state 和关闭重试地址保持，不调用 release_after_agents 当作已完成。只有计数回零后才允许现关闭链释放 Host-owned 地址；不新增遥测生命周期队列或 TelemetryState。
5. 不能“普通 observer task abort 后顺带 drop collector”。回归必须在 blocking job 已提交但原 globals 尚未发布的 barrier 暂停，分别取消 RPC waiter、关闭输入端 EOF、发起 Host close：counter 仍 pending、close 仍未成功、无新 init 被接纳；释放 barrier 后原 job/rollback 真结束，原 collector 只结算一次，close 重试才能成功。barrier 只在内部测试，不新增 SDK wire 操作。

`spawn_blocking` 的 runtime handle 可见性及在失败清理时仍有 Tokio exporter 批任务运行必须实际验证；本方案不另建 runtime。取消不伪报阻塞工作停止，最终初始化/关闭事实仍来自原框架 METRICS/RUNTIME 和初始化锁。

## 必须验证的有限场景

- 当前三个原正向初始化反例在新 Host 成功；初始化前 false、成功后 true；错误类型/空 endpoint/service、错误 enable_console 在副作用前明确失败。
- 不兼容外部 subscriber 明确失败且无 METRICS/RUNTIME 发布、无清理死锁；构建失败可再试。
- 同进程并发初始化至多一个成功；重复 init、重复 shutdown、shutdown 后 status 保持既有一次性行为。
- 稳定 layer 的 `OpenTelemetrySpanExt::context/set_parent` 与同 trace parent/child、sampling、初始化前 span、跨 init span和 shutdown 边界按上述定义验证，无 invalid ID、panic 或静默改 parent。
- 正常 Host 同时保留 stderr/RUST_LOG；enable_console=false/true 下 ACP stdout 均只含合法协议消息。
- 三个语言使用同一本地 OTLP gRPC collector，正常 Agent 的 LLM/tool 路径生成真实记录，核对 service.name、llm.calls/tokens、tool.executions 和正确 span 关联。collector 基于测试范围内现锁定 tonic/opentelemetry-proto，若需新 test-only 依赖先列明，不添加生产依赖。
- 设置较长的 OTEL_METRIC_EXPORT_INTERVAL/OTEL_BSP_SCHEDULE_DELAY，记录生成后立即 shutdown，collector 在成功返回前实际收到该批指标/结束 span；不声称失败 shutdown 一定无损，也不扩大成未接受的网络可靠投递服务保证。

## 实施边界建议

框架独占 `src/telemetry.rs` 及其独立进程测试、正式 telemetry 文档；SDK 独占 `main.rs`、`facade/telemetry.rs`、三语言本场景和共享 collector fixture。SDK 原 settlement counter/close hook 的复用及注释、必要的小型通用 guard 可在 `state.rs` 与 `core_profile/mod.rs` 内完成，但不得为 telemetry另开 registry 或改变原 Run 状态权威。现有 ACP、Agent、Metrics 记录点与唯一 Host transport 全部复用。无需改 wire catalog、生成器 acceptance、业务 scheduler 或 npm/PyPI/Maven 发布。

## 官方依据

- [tracing-subscriber reload 限制](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/reload/index.html)：不将整个 OTel layer 放入 reload。
- [动态 Filter](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/filter/fn.dynamic_filter_fn.html)：启用判断留在稳定 layer 上，并避免静态 callsite 缓存。
- [PreSampledTracer](https://docs.rs/tracing-opentelemetry/0.28.0/tracing_opentelemetry/trait.PreSampledTracer.html)：必须沿用同一 SdkTracer 的采样与 ID，而非另写逻辑。
- [SdkMeterProvider shutdown](https://docs.rs/opentelemetry_sdk/0.27.1/opentelemetry_sdk/metrics/struct.SdkMeterProvider.html#method.shutdown)：成功关闭刷新并停止后续导出，重复调用返回错误，失败不保证全部已刷出。

上述官方页面已实际读取 HTTP 200，原页面和摘要固定于本证据目录；候选没有由此自动升级为已批准实现。
