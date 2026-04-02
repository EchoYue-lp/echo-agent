//! # echo-core
//!
//! 核心 trait 和类型定义，作为 echo-agent 框架的基础层。
//!
//! 本 crate 提供所有子 crate 共享的接口和类型：
//! - [`error`]: 统一错误类型
//! - [`tools`]: 工具系统 trait (Tool, TypedTool) 和权限模型
//! - [`llm`]: LLM 客户端 trait 和消息类型
//! - [`agent`]: Agent trait、回调接口和事件
//! - [`guard`]: 护栏系统 trait 和管理器
//! - [`audit`]: 审计日志 trait 和事件类型

pub mod agent;
pub mod audit;
pub mod error;
pub mod guard;
pub mod llm;
pub mod retry;
pub mod tokenizer;
pub mod tools;
