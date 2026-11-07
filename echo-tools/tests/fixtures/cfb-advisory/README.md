# CFB／OLE2 解析回归样本

两份样本来自 calamine 上游 PR762 的修复提交
`b8ec4f09cba6aea7d8359ababc240c9ed064751b`，用于复验
`RUSTSEC-2026-0436`。文件名保留上游名称，内容未修改，来源遵循 calamine 的 MIT 许可。

- `cfb_dir_short_chunk_panic.xls`：目录尾部不足128字节；正确实现仍能读取 `Sheet1`。
- `cfb_difat_oom.xls`：DIFAT自循环；正确实现必须有界返回错误。

测试通过公开的 `Xls::new` 读取器执行；同一测试源进入框架 `media` 测试入口。
升级验证先在旧版仅执行短目录用例取得实际失败；旧版OOM样本仅做源码核查，
不主动制造无界分配。修复版本再执行两项完整测试。

逐文件Git blob、SHA256和上游API来源记录在本仓 ignored
`.echo-semantic/local/calamine-advisory-validation-20261104-r01/inputs.json`。
该处验证不代表当前App或SDK精pin已经采用新依赖。
