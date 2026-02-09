# echo-agent
一个rust实现的agent框架

## 该框架将实现如下核心 Agent 流程
* tools
* todo task
* human in loop
* subagent
* context compact
* mcp
* skills

## 该框架将支持如下功能
* 用户自由选择是否启用上述核心 agent 流程
* 支持多种使用方式，计划支持：命令行、HTTP
* 支持异步执行，让工具支持异步执行
* 友好的日志处理与错误处理
* 流式支持
* 持久化存储
* 并行工具执行
* 中间件系统，在工具执行前后增加钩子，方便做日志、监控、限流等