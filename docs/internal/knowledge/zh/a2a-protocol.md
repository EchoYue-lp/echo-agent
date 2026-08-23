# A2A 协议 (Agent-to-Agent Protocol)

本文档介绍 Google 提出的 A2A (Agent-to-Agent) 协议及 echo-agent 的实现。

---

## 概述

A2A 协议是一个开放标准，用于不同 Agent 框架之间的互操作。它定义了：

- **Agent Card**：Agent 能力描述卡片
- **任务生命周期**：任务状态机
- **消息格式**：标准化的请求/响应
- **流式事件**：SSE 实时推送

---

## 核心概念

### Agent Card

Agent Card 是 Agent 的"名片"，描述其能力和端点。

```json
{
  "name": "translator",
  "description": "多语言翻译 Agent",
  "version": "1.0.0",
  "url": "http://localhost:8080",
  "capabilities": {
    "streaming": true,
    "pushNotifications": false
  },
  "skills": [
    {
      "name": "translate",
      "description": "翻译文本到指定语言",
      "input": {
        "type": "object",
        "properties": {
          "text": { "type": "string" },
          "target_lang": { "type": "string" }
        }
      }
    }
  ]
}
```

### 任务状态机

```
┌─────────────────────────────────────────────────────────────────────┐
│                     Task State Machine                               │
│                                                                      │
│                        submitted                                     │
│                           │                                         │
│                           ▼                                         │
│                        working ──────────────────┐                  │
│                           │                      │                   │
│              ┌────────────┼────────────┐        │                   │
│              │            │            │        │                   │
│              ▼            ▼            ▼        │                   │
│          completed    failed    input-required │                   │
│              │            │            │       │                   │
│              │            │            └───────┘                   │
│              │            │                                        │
│              ▼            ▼                                        │
│           (终态)      (终态)                                        │
│                                                                      │
│   任何非终态 ──────────▶ canceled                                   │
└─────────────────────────────────────────────────────────────────────┘
```

| 状态 | 说明 |
|------|------|
| `submitted` | 任务已提交，等待处理 |
| `working` | 正在执行 |
| `input-required` | 需要用户输入后继续 |
| `completed` | 执行完成（终态） |
| `failed` | 执行失败（终态） |
| `canceled` | 已取消（终态） |

---

## echo-agent 实现

### A2AServer

```rust
// src/a2a/server.rs
pub struct A2AServer {
    card: AgentCard,
    agent: Box<dyn Agent>,
}

impl A2AServer {
    pub fn new(card: AgentCard, agent: impl Agent + 'static) -> Self {
        Self {
            card,
            agent: Box::new(agent),
        }
    }
    
    /// 处理同步请求
    pub async fn handle_request(&mut self, request: A2ARequest) -> Result<A2AResponse> {
        match request.method.as_str() {
            "tasks/send" => self.handle_send(request).await,
            "tasks/get" => self.handle_get(request).await,
            "tasks/cancel" => self.handle_cancel(request).await,
            _ => Err(A2AError::MethodNotFound),
        }
    }
    
    /// 处理流式请求（SSE）
    pub async fn handle_request_stream(
        &mut self,
        request: A2ARequest,
    ) -> Result<impl Stream<Item = Result<A2AStreamEvent>>> {
        // 执行 agent 并产生流式事件
    }
}
```

### A2AClient

```rust
// src/a2a/client.rs
pub struct A2AClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl A2AClient {
    /// 发现远程 Agent
    pub async fn discover(&self) -> Result<AgentCard> {
        let url = format!("{}/.well-known/agent.json", self.base_url);
        let card = self.http_client.get(&url).send().await?.json().await?;
        Ok(card)
    }
    
    /// 发送任务
    pub async fn send_task(&self, message: &str) -> Result<String> {
        let request = A2ARequest {
            method: "tasks/send".into(),
            params: json!({ "message": message }),
        };
        let response = self.http_client
            .post(&format!("{}/tasks/send", self.base_url))
            .json(&request)
            .send().await?
            .json().await?;
        Ok(response.result["task_id"].as_str().unwrap().to_string())
    }
    
    /// 订阅流式事件
    pub async fn subscribe(&self, task_id: &str) -> Result<impl Stream<Item = A2AStreamEvent>> {
        // SSE 连接
    }
}
```

### AgentCard Builder

```rust
// src/a2a/types.rs
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub version: String,
    pub url: String,
    pub capabilities: AgentCapabilities,
    pub skills: Vec<AgentSkill>,
}

impl AgentCard {
    pub fn builder(name: &str, url: &str) -> AgentCardBuilder {
        AgentCardBuilder::new(name, url)
    }
}

pub struct AgentCardBuilder {
    card: AgentCard,
}

impl AgentCardBuilder {
    pub fn description(mut self, desc: &str) -> Self {
        self.card.description = desc.to_string();
        self
    }
    
    pub fn version(mut self, v: &str) -> Self {
        self.card.version = v.to_string();
        self
    }
    
    pub fn skill(mut self, skill: AgentSkill) -> Self {
        self.card.skills.push(skill);
        self
    }
    
    pub fn streaming(mut self) -> Self {
        self.card.capabilities.streaming = true;
        self
    }
    
    pub fn build(self) -> AgentCard {
        self.card
    }
}
```

---

## 使用示例

### 服务端：发布 Agent

```rust
use echo_agent::a2a::{AgentCard, AgentSkill, A2AServer};
use echo_agent::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // 创建 Agent
    let agent = ReactAgentBuilder::simple("qwen3-max", "翻译助手")?;
    
    // 定义 Agent Card
    let card = AgentCard::builder("translator", "http://localhost:8080")
        .description("多语言翻译 Agent，支持中英日韩")
        .version("1.0.0")
        .skill(AgentSkill::new("translate", "翻译文本到指定语言"))
        .skill(AgentSkill::new("detect_language", "检测文本语言"))
        .streaming()
        .build();
    
    // 启动服务器
    let server = A2AServer::new(card, agent);
    server.serve("0.0.0.0:8080").await?;
    
    Ok(())
}
```

### 客户端：发现和调用

```rust
use echo_agent::a2a::A2AClient;

#[tokio::main]
async fn main() -> Result<()> {
    let client = A2AClient::new("http://localhost:8080");
    
    // 发现 Agent
    let card = client.discover().await?;
    println!("发现 Agent: {} - {}", card.name, card.description);
    println!("技能列表:");
    for skill in &card.skills {
        println!("  - {}: {}", skill.name, skill.description);
    }
    
    // 发送任务
    let task_id = client.send_task("将以下文本翻译成日语：你好世界").await?;
    println!("任务已提交: {}", task_id);
    
    // 获取结果
    let result = client.get_task(&task_id).await?;
    println!("结果: {:?}", result);
    
    Ok(())
}
```

### 流式订阅

```rust
// 订阅实时事件
let mut stream = client.subscribe(&task_id).await?;

while let Some(event) = stream.next().await {
    match event? {
        A2AStreamEvent::StatusUpdate { status, .. } => {
            println!("状态更新: {:?}", status);
        }
        A2AStreamEvent::Artifact { content, .. } => {
            print!("{}", content);  // 流式输出
        }
        A2AStreamEvent::Completed { result } => {
            println!("\n完成: {}", result);
            break;
        }
    }
}
```

---

## API 端点

| 端点 | 方法 | 说明 |
|------|------|------|
| `/.well-known/agent.json` | GET | 获取 Agent Card |
| `/tasks/send` | POST | 发送任务（同步） |
| `/tasks/sendSubscribe` | POST | 发送任务（流式 SSE） |
| `/tasks/get` | POST | 查询任务状态 |
| `/tasks/cancel` | POST | 取消任务 |

### 请求/响应格式

**发送任务：**

```json
// POST /tasks/send
{
  "jsonrpc": "2.0",
  "method": "tasks/send",
  "params": {
    "message": "翻译：你好",
    "skill": "translate"
  },
  "id": "req-123"
}

// Response
{
  "jsonrpc": "2.0",
  "result": {
    "task_id": "task-456",
    "status": "completed",
    "artifacts": [
      { "type": "text", "content": "Hello" }
    ]
  },
  "id": "req-123"
}
```

---

## 与其他协议对比

| 协议 | 定位 | 特点 |
|------|------|------|
| **A2A** | Agent 间通信 | 标准化状态机、Agent Card |
| **MCP** | 工具/资源访问 | 工具发现、资源访问 |
| **OpenAI Functions** | LLM 工具调用 | 简单、广泛支持 |
| **LangChain Tools** | 应用内工具 | 无跨框架标准 |

### A2A vs MCP

- **A2A**：Agent 发现和任务委托
- **MCP**：工具和资源访问

两者互补：

```rust
// Agent A 通过 A2A 调用 Agent B
// Agent B 通过 MCP 访问外部工具

let a2a_client = A2AClient::new("http://agent-b:8080");
let result = a2a_client.send_task("分析数据").await?;
```

---

## 实际应用场景

### 1. 多 Agent 协作

```
┌─────────────┐     A2A      ┌─────────────┐     A2A      ┌─────────────┐
│ Orchestrator│─────────────▶│ Researcher  │─────────────▶│   Writer    │
└─────────────┘              └─────────────┘              └─────────────┘
      │                            │                            │
      │                            │ MCP                        │ MCP
      ▼                            ▼                            ▼
┌─────────────┐              ┌─────────────┐              ┌─────────────┐
│   Planner   │              │ Web Search  │              │ File System │
└─────────────┘              └─────────────┘              └─────────────┘
```

### 2. 跨框架互操作

```
┌───────────────────────────────────────────────────────────────┐
│                     A2A Gateway                               │
│                                                               │
│  ┌─────────┐     ┌─────────┐     ┌─────────┐                 │
│  │echo-agent│     │LangChain│     │ CrewAI  │                 │
│  │ (Rust)  │     │(Python) │     │(Python) │                 │
│  └────┬────┘     └────┬────┘     └────┬────┘                 │
│       │               │               │                       │
│       └───────────────┴───────────────┘                       │
│                       │                                       │
│                  A2A Protocol                                 │
└───────────────────────────────────────────────────────────────┘
```

---

## 参考资料

- [A2A Protocol 规范](https://github.com/google/A2A)
- [Google A2A 公告](https://blog.google/technology/ai/agent-to-agent-protocol/)
- [Agent Card 规范](https://github.com/google/A2A/blob/main/specification.md#agent-card)