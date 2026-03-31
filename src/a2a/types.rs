//! A2A 协议类型定义
//!
//! 遵循 Google A2A 协议规范定义 Agent Card、Task 等类型。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ── Agent Card ───────────────────────────────────────────────────────────────

/// Agent Card — 描述 Agent 的能力和接口（A2A 规范核心类型）
///
/// 通过 `/.well-known/agent.json` 端点发布，供其他 Agent 发现和调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    /// Agent 名称
    pub name: String,
    /// Agent 描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Agent 服务的 URL 端点
    pub url: String,
    /// Agent 版本
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Agent 提供者信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    /// Agent 技能列表
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
    /// 支持的输入内容类型
    #[serde(default = "default_content_types")]
    pub default_input_modes: Vec<String>,
    /// 支持的输出内容类型
    #[serde(default = "default_content_types")]
    pub default_output_modes: Vec<String>,
    /// 认证方式
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<AgentAuthentication>,
    /// 额外能力标记
    #[serde(default)]
    pub capabilities: AgentCapabilities,
}

fn default_content_types() -> Vec<String> {
    vec!["text/plain".to_string()]
}

impl AgentCard {
    /// 创建 AgentCard 构建器
    pub fn builder(name: impl Into<String>, url: impl Into<String>) -> AgentCardBuilder {
        AgentCardBuilder {
            name: name.into(),
            url: url.into(),
            description: None,
            version: None,
            provider: None,
            skills: Vec::new(),
            default_input_modes: default_content_types(),
            default_output_modes: default_content_types(),
            authentication: None,
            capabilities: AgentCapabilities::default(),
        }
    }

    /// 从已有的 Agent trait 对象自动生成 Agent Card
    pub fn from_agent(
        agent: &dyn crate::agent::Agent,
        url: impl Into<String>,
    ) -> Self {
        let skills: Vec<AgentSkill> = agent
            .tool_definitions()
            .into_iter()
            .map(|td| {
                AgentSkill::new(&td.function.name, &td.function.description)
            })
            .collect();

        AgentCard {
            name: agent.name().to_string(),
            description: Some(agent.system_prompt().to_string()),
            url: url.into(),
            version: Some("1.0.0".to_string()),
            provider: None,
            skills,
            default_input_modes: default_content_types(),
            default_output_modes: default_content_types(),
            authentication: None,
            capabilities: AgentCapabilities::default(),
        }
    }
}

/// Agent Card 构建器
pub struct AgentCardBuilder {
    name: String,
    url: String,
    description: Option<String>,
    version: Option<String>,
    provider: Option<AgentProvider>,
    skills: Vec<AgentSkill>,
    default_input_modes: Vec<String>,
    default_output_modes: Vec<String>,
    authentication: Option<AgentAuthentication>,
    capabilities: AgentCapabilities,
}

impl AgentCardBuilder {
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    pub fn provider(mut self, provider: AgentProvider) -> Self {
        self.provider = Some(provider);
        self
    }

    pub fn skill(mut self, skill: AgentSkill) -> Self {
        self.skills.push(skill);
        self
    }

    pub fn skills(mut self, skills: Vec<AgentSkill>) -> Self {
        self.skills.extend(skills);
        self
    }

    pub fn input_modes(mut self, modes: Vec<impl Into<String>>) -> Self {
        self.default_input_modes = modes.into_iter().map(|m| m.into()).collect();
        self
    }

    pub fn output_modes(mut self, modes: Vec<impl Into<String>>) -> Self {
        self.default_output_modes = modes.into_iter().map(|m| m.into()).collect();
        self
    }

    pub fn authentication(mut self, auth: AgentAuthentication) -> Self {
        self.authentication = Some(auth);
        self
    }

    pub fn streaming(mut self) -> Self {
        self.capabilities.streaming = true;
        self
    }

    pub fn push_notifications(mut self) -> Self {
        self.capabilities.push_notifications = true;
        self
    }

    pub fn build(self) -> AgentCard {
        AgentCard {
            name: self.name,
            description: self.description,
            url: self.url,
            version: self.version,
            provider: self.provider,
            skills: self.skills,
            default_input_modes: self.default_input_modes,
            default_output_modes: self.default_output_modes,
            authentication: self.authentication,
            capabilities: self.capabilities,
        }
    }
}

/// Agent 提供者信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProvider {
    /// 组织/公司名称
    pub organization: String,
    /// 联系方式 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl AgentProvider {
    pub fn new(organization: impl Into<String>) -> Self {
        Self {
            organization: organization.into(),
            url: None,
        }
    }

    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// Agent 技能描述
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    /// 技能 ID
    pub id: String,
    /// 技能名称
    pub name: String,
    /// 技能描述
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 示例输入
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
    /// 支持的输入类型
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_modes: Vec<String>,
    /// 支持的输出类型
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub output_modes: Vec<String>,
    /// 自定义标签
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl AgentSkill {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        let name_str: String = name.into();
        Self {
            id: name_str.clone(),
            name: name_str,
            description: Some(description.into()),
            examples: Vec::new(),
            input_modes: Vec::new(),
            output_modes: Vec::new(),
            tags: Vec::new(),
        }
    }

    pub fn with_examples(mut self, examples: Vec<impl Into<String>>) -> Self {
        self.examples = examples.into_iter().map(|e| e.into()).collect();
        self
    }

    pub fn with_tags(mut self, tags: Vec<impl Into<String>>) -> Self {
        self.tags = tags.into_iter().map(|t| t.into()).collect();
        self
    }
}

/// Agent 认证配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAuthentication {
    /// 认证方案列表
    pub schemes: Vec<AuthenticationScheme>,
}

/// 认证方案
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationScheme {
    /// 方案类型: "apiKey", "bearer", "oauth2" 等
    pub scheme: String,
    /// 附加配置
    #[serde(flatten)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Agent 能力标记
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    /// 是否支持流式输出
    #[serde(default)]
    pub streaming: bool,
    /// 是否支持推送通知
    #[serde(default)]
    pub push_notifications: bool,
    /// 是否支持会话状态
    #[serde(default)]
    pub state_transition_history: bool,
}

// ── A2A Task 类型 ────────────────────────────────────────────────────────────

/// A2A 任务请求
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskRequest {
    /// JSON-RPC 版本
    pub jsonrpc: String,
    /// 请求 ID
    pub id: String,
    /// 方法名
    pub method: String,
    /// 参数
    pub params: A2ATaskParams,
}

/// A2A 任务参数
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskParams {
    /// 任务 ID（可选，新任务可省略）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 会话 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 消息内容
    pub message: A2AMessage,
}

/// A2A 消息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AMessage {
    /// 角色: "user" 或 "agent"
    pub role: String,
    /// 消息部分列表
    pub parts: Vec<A2APart>,
}

/// 消息内容部分
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum A2APart {
    /// 文本内容
    #[serde(rename = "text")]
    Text { text: String },
    /// 文件内容
    #[serde(rename = "file")]
    File {
        #[serde(rename = "mimeType")]
        mime_type: String,
        data: String,
    },
}

impl A2AMessage {
    /// 创建用户文本消息
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            parts: vec![A2APart::Text { text: text.into() }],
        }
    }

    /// 创建 Agent 文本消息
    pub fn agent_text(text: impl Into<String>) -> Self {
        Self {
            role: "agent".to_string(),
            parts: vec![A2APart::Text { text: text.into() }],
        }
    }

    /// 获取所有文本内容
    pub fn text_content(&self) -> String {
        self.parts
            .iter()
            .filter_map(|p| match p {
                A2APart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A2A 任务响应
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskResponse {
    /// JSON-RPC 版本
    pub jsonrpc: String,
    /// 请求 ID
    pub id: String,
    /// 任务结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<A2ATask>,
    /// 错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<A2AError>,
}

/// A2A 任务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATask {
    /// 任务 ID
    pub id: String,
    /// 会话 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 任务状态
    pub status: A2ATaskStatus,
    /// 消息历史
    #[serde(default)]
    pub history: Vec<A2AMessage>,
    /// Agent 产出的成果
    #[serde(default)]
    pub artifacts: Vec<A2AArtifact>,
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2ATaskStatus {
    /// 状态: submitted, working, input-required, completed, canceled, failed
    pub state: String,
    /// 状态消息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<A2AMessage>,
}

/// Agent 产出
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct A2AArtifact {
    /// 产出名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 产出内容部分
    pub parts: Vec<A2APart>,
}

/// A2A 错误
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2AError {
    pub code: i32,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_card_builder() {
        let card = AgentCard::builder("test-agent", "http://localhost:8080")
            .description("测试 Agent")
            .version("1.0.0")
            .skill(AgentSkill::new("calc", "数学计算"))
            .streaming()
            .build();

        assert_eq!(card.name, "test-agent");
        assert_eq!(card.description.as_deref(), Some("测试 Agent"));
        assert_eq!(card.skills.len(), 1);
        assert!(card.capabilities.streaming);
    }

    #[test]
    fn test_agent_card_serialization() {
        let card = AgentCard::builder("test", "http://localhost")
            .skill(AgentSkill::new("echo", "回声"))
            .build();

        let json = serde_json::to_string_pretty(&card).unwrap();
        assert!(json.contains("\"name\":"));
        assert!(json.contains("\"skills\":"));

        // 反序列化
        let parsed: AgentCard = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
    }

    #[test]
    fn test_a2a_message() {
        let msg = A2AMessage::user_text("你好");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.text_content(), "你好");
    }

    #[test]
    fn test_agent_skill() {
        let skill = AgentSkill::new("translate", "翻译")
            .with_tags(vec!["nlp", "translation"])
            .with_examples(vec!["翻译'你好'为英文"]);

        assert_eq!(skill.id, "translate");
        assert_eq!(skill.tags.len(), 2);
        assert_eq!(skill.examples.len(), 1);
    }
}
