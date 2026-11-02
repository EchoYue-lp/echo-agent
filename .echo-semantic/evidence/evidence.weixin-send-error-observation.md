---
schema_version: 1
id: evidence.weixin-send-error-observation
kind: evidence
title: 微信原发送诊断的源码接收与定向验证
observed_at: source:fe2e6ef2c0e588fc8262d4492054b169f68cf447605abefd0a2bf8b73e04bd07
source_refs:
- echo-integration/src/channels/channels/weixin/api.rs
- echo-integration/src/channels/channels/weixin/api/send_diagnostic.rs
- tests/weixin_send_diagnostic.rs
- echo-integration/docs/weixin-business-acceptance.md
- docs/adr/0099-personal-weixin-channel.md
- Cargo.toml
supports:
- rule.framework-layer-ownership
limitations:
- 本记录覆盖6个private helper、6个root实际回环集成方法和3个原ACK/Unknown方法的定向验证，不能扩大为完整矩阵。
- 已验证本地合成HTTP及实际EnvFilter/fmt捕获；未验证EKO app.log启动、真实微信账号、手机实收或现场旧-2的业务原因。
- Source/final源码治理结果不替代后继focused、完整工程矩阵、消费者或main CI资格。
---

# 微信原发送诊断的源码接收与定向验证

## 支持的结论

`echo-integration/src/channels/channels/weixin/api.rs` 沿原一次HTTP发送及原
`WeixinSendOutcome`记录旁路诊断。`execute_json_observed`保原transport/HTTP/body/decode
第一错误，ret原优先规则、Accepted/Rejected/Unknown、Unknown不授权重试以及原CLI
通知settlement均不变。non2xx仍先返回原HTTP错误，未捕获body只记不可得。

新私有helper `echo-integration/src/channels/channels/weixin/api/send_diagnostic.rs`
只投影已取得status、ret/errcode各自类型与值、受限errmsg形状/摘要及真实响应ID来源。
本地client_id仅单列SHA，不生成provider requestId；未知正文、原header/Auth/recipient/
context/clientID和UTF-8私密字面用于内存排除，不写完整body/headers/URL或任意Debug。
它不拥有业务许可、重试、store、执行或结算状态。

原错误producer target仍为`echo_agent::channels::weixin::send_observation`。
新`tests/weixin_send_diagnostic.rs`定义原`for_local_auth_fixture`→原`send_text` POST→
原错误WARN→async scoped Dispatch→真实EnvFilter/fmt→32KiB writer的回环控制；
包括Accepted不发错误trace、严格过滤不等于没HTTP、原non2xx/截断/Unknown首错及隐私负控。
每case实际通过server join、请求数1和原请求字段完整断言，全部身份/响应是合成夹具。
最终源码上真实list与执行为6个private helper通过、6个root方法通过，以及3个原ACK/Unknown
方法分别exact1通过，全部0失败/0忽略、自然0。16是root六方法内部场景数，不冒16个selected。
原默认info过滤能捕获真实错误WARN，严格error过滤仍实际POST一次而writer为空；Accepted
无错误trace。non2xx/截断/Unknown及原header/Auth/正文/身份/UTF-8回显负控都已实际通过。

根`Cargo.toml`只为同version/path `echo_integration`添加既有`channel-test-support`
的dev边；normal边、`testing=[]`和resolver3保持原值。静态TOML去掉该项后与HEAD结构相等，
lock1442与baselinea5a6原字节不变。根`--no-default-features --test weixin_send_diagnostic`
实际NoRun自然0，CargoJSON记录root/test features为`[]`、同path integration为
`channel-test-support,channels,default`；不因dev测试可用而宣称production release已验。

## 来源与范围

正式接收基于a8a8511的精五文件；IM最终测试SHA为
`4edb156dc77c09e87aea969417dddf1dc15ac518a335477ffe5f2f333aa4bd84`。
原producer/api和测试精bytes保持。helper原500d候选在实际Clippy中因两处collapsible_if
退出101，随后只合并相同条件链，最终helper SHA为
`e46cc383a044a919deff2dc6ab5fd7821236f77b4acfce801423c80d51ee4fb6`。
修订后重新NoRun/list并执行上述6+6+3方法全绿，原101和修订前后程序不覆盖。
原补丁静态票保留；最终定向integration/root-test Clippy `-D warnings`自然0，
整仓cargo fmt write及check自然0、输出0字节、源码无格式漂移。
IM新增测试独立静态复审C0/I0/M0，其原review/格式/应用票保留在作者交件中。

本批原caller/raw/receipt/run、actual list、源前后guard与程序/FP/.d、原profile JSON保存在
本仓ignored `.echo-semantic/local/framework-native-diagnostics-focused-20261102-r01/`。
最终root程序SHA `5f62fbd738d7fbc1d3cd9de63a0fb497c01488c38e77bef6c86ed291cf30150a`，
private程序SHA `3246a4e10b92600cdd58b41ad49f6ec3db6b237c4cd5f9b1d307e5aafebc8c28`；
原fingerprint profile十进制`5631241865842084492`及原JSON字节均已保全。
该批同时验证Driver的55方法；它们不加进微信15方法，也不替代App真实多turn/Cron验收。

正常原Source-r01实际exit1，raw SHA256 `dbf89ea11f14b065980425b18640c1e409080fd249c56e529aa09ff4dd0e5eaf`，未截断。
六项依次为：已有差异策略未绑定；机器publicApi、newStateAuthority标志未声明；
上述新helper和root测试高风险路径无有效语义文本；未绑定正式design/ADR。
原调用、raw、receipt和run位于本仓ignored
`.echo-semantic/local/framework-weixin-error-observation-adoption-20261102-r01/source-r01/`。
该原失败不改写为成功；后继正常Source/final按其自己的原始收据与contentDigest判断。

本次正常预检按机器高风险覆盖标志保守声明publicApi/newStateAuthority，并绑定既有
`docs/adr/0099-personal-weixin-channel.md`。机器启发式命中的是helper的`pub(super)`及测试
Capture等可变记录；实际没有新增生产public API、协议、业务状态权威或第二owner。
此Evidence记录准确private/test边界，不把诊断结构当业务状态，也不改变Map、外部工具、
schema、exclusion、依赖风险或原受控接受集。原Source、预检和inventory失败原件均保留。

## 已知缺口

中文微信验收页同步解释本私有诊断。公开调用形状未变，examples无需改源码，其可执行检查
仍属后继工程门禁；官网不据静态观测宣称手机投递改善，当前无对应展示改动。
当前已取得上述Root具名focused窗口结果；更新本记录后须按新组合源重核Source/final，
旧六路径source-only通过只保原切点资格。旧整仓工程票与本次新增source/dev边分开保留。
