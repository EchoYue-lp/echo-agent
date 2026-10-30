# DeepSeek Anthropic 成功工具的默认非错误表示

Frame已明确采用本次最小映射，修复真实publisher的成功工具回填；本增量不改Core观察、State、tool_support、模型族或自动重试。原metadata-r01/r02与compat包只读，metadata仍另层采用。

## 三种canonical观察与wire转换

| Message.tool_result_is_error | Canonical/State | DeepSeek Anthropic wire |
| --- | --- | --- |
| None | 保未知，不推断执行成功 | 原普通tool_result，is_error省略 |
| Some(false) | 保真实成功产生的false，serde/State不丢 | 显式转换为普通非错误tool_result的默认表示，is_error省略 |
| Some(true) | 保真实错误观察 | HTTP前明确报无法兑现native错误语义，不自动重试 |

真正Anthropic端点继续原样编码显式false/true，本规则只适用于既有精确DeepSeek host/path判定。普通结果表示false不等于把所有缺字段的canonical观察都判成成功，也不意味着整个Agent/TaskRun完成。

这项wire省略是有据可审的等价转换，不是无声丢弃观察：真实publisher成功路径总写Some(!ToolResult.success)，因此canonical仍可证明false；发送时只用协议普通非错误结果表达同一含义。Some(true)不能靠普通content或中文错误文案冒native错误标志，所以仍明确拒绝。

## 固定primary

官方Python SDK提交 `202db72c87d7c6eaa72129c67ce67fd343a3a20c` 的 ToolResultBlockParam 以total=False定义可选is_error。该声明只证明可选，不能单独证明默认false。

同一固定提交的 [官方同步/异步tool runner](https://github.com/anthropics/anthropic-sdk-python/blob/202db72c87d7c6eaa72129c67ce67fd343a3a20c/src/anthropic/lib/tools/_beta_runner.py) 在正常tool.call返回时发送普通tool_result且省略is_error，在ToolError/Exception分支显式发送true。由这两个vendor-owned实际路径可核对“省略为普通非错误结果”的协议使用方式；本说明没有伪造JSON Schema的default注解，也没有声称live模型通过。

[DeepSeek官方兼容文档](https://api-docs.deepseek.com/zh-cn/guides/anthropic_api) 列明tool_result的id/content支持、is_error被忽略，且以普通Messages SDK接入。完整原件/HTTP/hash、官方runner blob SHA及成功/错误行段在primary-contract-evidence.json与references/。作用范围只覆盖该已知协议差异，不把它推广到未知兼容服务。

## 测试责任

协议本crate测试逐None/false/true及流/非流检查wire表示、HTTP前拒绝和canonical值未变；这些只能证明adapter，不足以证明真实publisher。根/EKO消费者控制必须调用真实publish_completed_call，成功false经原State投影/restore后进入下一HTTP请求；业务错误true即使Rust Result为Ok也须保true并使第二次HTTP零调用。None为合法未知来源输入的另一个正控，不能通过清除真实publisher false构造。

根/consumer作者复用既有Framework→integration依赖，禁止为此向integration引入Root循环依赖或新增public client注入API。本线只交源码和测试定义，实际同源编译、publisher→HTTP、协议回归与live资格由Root/Frame准入后分别取证；旧仅默认None的unsigned测试不再被当作publisher闭合证据。
