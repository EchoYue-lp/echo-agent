# Responses 流诊断日志

当 Responses 流解析与真实供应商行为不一致时，框架在 `echo_agent::provider_diagnostics` target 输出 `responses_diagnostic` JSON 字段。它只记录观察，不改变错误分类、重试、phase 校验、历史重放或运行时完成判定。

每个请求的 `trace_id` 在请求、事件和收尾中保持一致。原生 response/item ID 只记录 SHA-256，便于关联 checkpoint 中的 ID，同时避免输出原始标识。日志包含三类证据：

- `request`：供应商配置身份、模型、解析后的 dialect、端点 scheme/host/port、实际请求 JSON 的流式 SHA-256 与字节数、消息角色/内容类型统计、工具名及工具/schema 指纹、显式思考/采样/输出限制和关键开关。端点 userinfo、路径、query、fragment 不输出。
- `event`：收到事件的本地顺序、供应商 sequence_number、output/content/summary index、item 类型和状态、前后 phase。phase 区分缺失、null、空字符串和未知枚举，最多记录64个 Unicode 字符，并保留完整值的指纹和字符数。正文、推理文本、图片内容、工具参数及供应商错误正文不输出。
- 收尾：传输启动失败、后续传输失败、适配器拒绝、`[DONE]`、EOF 或消费者丢弃流。失败收尾重复包含请求形状和最后8个事件，因此即使 INFO 被过滤，ERROR 仍保留此次冲突的关联字段。

常规事件最多记录96项；token delta 只记录每个事件类型/坐标的首次出现。后续 token 帧累计到计数，不逐 token 写日志。phase 变化和供应商终态仍会记录，且收尾保留最后8个事件。工具形状及完整响应的输出列表各最多128项。请求和 schema 计算指纹时不会把正文再次复制成完整 JSON 缓冲区。

## 本地启用与读取

日志使用框架级 target，因此宿主默认只开启 `echo_agent=info` 时也能看到。需要单独启用时，在启动宿主前设置：

```sh
RUST_LOG=echo_agent::provider_diagnostics=info
```

若还需要宿主的会话/turn 路径，可将其既有 target 加到同一个过滤表达式。不要为了诊断开启整个 HTTP 客户端的 trace 或打印 Authorization 头。

Eclip 的既有文件 sink 写入 `$EKO_DATA_DIR/logs/app.log`，未指定数据目录时使用其默认用户目录。测试包启动器会保留原数据目录，并显式开启该诊断 target；只需按原模型配置新建会话发送“你好”。记录测试时间即可关联 `trace_id`。

发现 `Responses raw phase changed` 后，查找对应的 `stage=adapter_error`，读取 `detail.last_events` 中的 `previous_phase`、`item.phase`、`event_type` 和 `output_index`。不能仅凭错误名称认定供应商返回非法数据，也不能将 `provider_terminal_seen=true` 当作运行时任务成功。

## 验证与范围

隐私负控覆盖认证值、URL 凭证、消息正文、图片、工具描述/schema 默认值和用户标识；适配器回归覆盖冲突前后 phase 可关联且仍返回原错误。事件限量及 Unicode phase 截断另有控制。

本次只为定位真实协议差异增加内部日志，没有新增公共 API、配置项或依赖。现有 examples 的调用方式及官网能力描述不变，因此无需修改示例或官网；这不构成供应商兼容性修复或 GUI 验收。
