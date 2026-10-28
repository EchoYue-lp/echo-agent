# 微信当前poll业务接受与最小协议观测

当前官方Tencent/openclaw-weixin874637a0、npm2.4.10定义ret可选，明确非0ret/errcode
拒绝；正常getupdates不补ret。支持合法缺ret的typed msgs/cursor是当前协议正确性，
不是本系统旧schema兼容；deprecated sync_buf不接入。

解码先要求object，typed字段复用原Serde解码；缺失和显式null分开，duplicate field
及坏类型拒绝，非0/-14优先且不交付msgs/cursor。只有无错误/无null码，且明确业务0或
当前typed msgs数组/get_updates_buf字符串，才建立接受；空/无关object、仅timeout
hint、数组、null和坏类型均不Ready。HTTP2xx本身不提供业务接受。

原poll/send/ingress owner、cursor frontier/CAS、去重、Stop和重试不变；不新建Store、
poller、模型或协议双模式。上述poll切片未改sender接受谓词；后继发送ACK修复见下文。

原client execute_json仅对原POST getupdates增加一次脱敏诊断；clone共享单次额度，
不是运行模式开关或新owner。只记实际status、完整受限响应bytes长度/SHA、允许字段
presence/type、i64业务码、msgs数量和未知键数量，不遍历msgs，不输出未知键名、
headers/token/cursor/errmsg/body。非2xx仍原样拒绝，未捕获body明确记未捕获。

本轮已正式整合：5个作者parser tests/19参数cases在原生产实际3通过2失败，再切当前谓词；
同parser命令与原HTTP optional-ret正控已取得GREEN。诊断、原通道与ACK/cursor/Stop保留回归
按各自源码快照单独记录；这些focused结果不授新App或真实收发资格。现场仅知typed decode无Some0且未判非0，不能由OptionNone
证明原键缺失而非null。原扫码授权与GUIStop保凭据不变；后继只由Root恢复同实例
同授权捕获一次安全形状，不手工getupdates或另起poller。


本轮真实原HTTP对照：仅省ret的新正控实际101/未到admission，而相同40条DATA/
HITL/ACK/cursor/Stop原ret0正控实际0（2.24秒）；支持当前predicate阻断，不是整套
fixture损坏。IM v2九负控保实际Stop前Unknown/Disconnected/evidence以及nonzero
优先null，未降低容量/期限、不把所有Unknown转正。原始RED与GREEN、后续保留回归分别
绑定实际源码，不回填旧失败。现场键presence/type/实际HTTPcode仍未捕获，不冒RootCause已证。


## 后继 sendMessage 缺 ret 的合法业务回执

本后继切片单独修复发送 ACK，不改写上文 poll 批次的历史范围。腾讯固定
874637a0／npm2.4.10 的 SendMessageResp.ret 可选，message_id 为无损字符串。
只有合法 object 中 ret 确实缺失、没有非零业务码或 raw null 错误码，且有原 typed
非空白 message_id，才新增 Accepted；非零业务码优先。原明确 ret=0 的 object
接受路径保留，原 string/u64 ID decoder 不改，不重解析字符串整数。

空／无关 object、raw null、array、坏类型、丢失或截断回执不能凭 HTTP200／None
变成 Accepted。Accepted 仍只是原请求的服务器业务回执，不是手机 Received；
不回写历史 Unknown，不授权自动重发。原发送 owner、冻结 context、client_id、
cursor／ACK、Stop 与退出收尾均不新增或替换，不引入 getConfig／typing 能力。

验收包含wire 5方法／33 raw与原HTTP 1方法／13场景。本轮在原生产上分别实际list5／1，
取得3通过2失败／0通过1失败的目标RED后，只采用最小wire生产补丁，再由同命令取得
5通过／1通过的GREEN。源码、原始退出与后续保留回归分别绑定，不回填早期失败。
静态审查不替代执行，focused与loopback HTTP结果不代表真实账号或手机Received。
