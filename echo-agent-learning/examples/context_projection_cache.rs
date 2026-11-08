//! 不调用模型的上下文版本示例；由 examples 编译链覆盖公开 API。
use echo_agent::compression::{ContextManager, ContextProjection};
use echo_agent::llm::types::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut context = ContextManager::builder(4096)
        .with_system("固定指令".into())
        .build();
    context.push(Message::user("原始请求".into()));
    for version in ["a", "a", "b", "a"] {
        context.apply_projection_scope(
            "example",
            &[ContextProjection::versioned(
                "runtime-state",
                version,
                Some(Message::user(format!("当前状态：{version}"))),
            )],
        );
    }
    if context.messages().len() != 5 || context.protected_message_count() != 1 {
        return Err("版本去重或最新保护不符合合同".into());
    }
    Ok(())
}
