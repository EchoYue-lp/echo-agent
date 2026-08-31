# Skill 系统设计

本文档介绍 echo-agent 的 Skill 系统设计，该系统对齐 [agentskills.io](https://agentskills.io/) 规范。

---

## 概述

Skill（技能）是比 Tool（工具）更高层的能力抽象。一个 Skill 可以包含多个相关工具 + 可选的系统提示词注入，形成完整的能力包。

### Skill vs Tool

| 维度 | Tool | Skill |
|------|------|-------|
| 粒度 | 单个原子操作 | 领域能力包（多工具 + 提示词） |
| 注册方式 | `agent.add_tool(box)` | `agent.add_skill(box)` |
| 提示词 | 无 | 可注入系统提示词 |
| 语义 | "做一件事" | "我精通某个领域" |

---

## 两种 Skill 类型

echo-agent 支持两种 Skill 类型：

### 1. Code-based Skill（代码型）

直接在 Rust 代码中定义，注册时立即生效。

```rust
// src/skills/mod.rs
pub trait Skill: Send + Sync {
    /// 唯一标识（小写，如 "calculator"）
    fn name(&self) -> &str;
    
    /// 人类可读描述
    fn description(&self) -> &str;
    
    /// 提供的工具列表（每次调用返回新实例）
    fn tools(&self) -> Vec<Box<dyn Tool>>;
    
    /// 可选：注入到系统提示词的文本
    fn system_prompt_injection(&self) -> Option<String> {
        None
    }
}
```

**示例：调研技能**

```rust
pub struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }

    fn description(&self) -> &str {
        "网页调研能力：搜索 + 抓取 + 摘要"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SearchTool),
            Box::new(FetchTool),
            Box::new(SummarizeTool),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("你拥有网页调研能力。当需要最新信息时，先用 search 检索，再 fetch 关键页面、最后 summarize 总结。".into())
    }
}

// 注册
agent.add_skill(Box::new(ResearchSkill));
```

### 2. File-based Skill（文件型）

通过 `SKILL.md` 文件定义，支持渐进式披露（Progressive Disclosure）。

**目录结构：**

```
skills/
├── code-review/
│   ├── SKILL.md          # 技能定义（必需）
│   └── references/
│       ├── checklist.md  # 参考文档
│       └── style_guide.md
│
├── data-analyst/
│   ├── SKILL.md
│   └── references/
│       └── statistical_methods.md
│
└── web-researcher/
    ├── SKILL.md
    └── references/
        ├── research_template.md
        └── source_evaluation.md
```

**SKILL.md 格式：**

```markdown
---
name: code-review
description: Comprehensive code review capability
metadata:
  version: "1.0.0"
  author: echo-agent
  tags: "code, review"
---

# Code Review Skill

You are an expert code reviewer. When reviewing code, consider:

1. **Correctness**: Does the code do what it's supposed to?
2. **Performance**: Are there any obvious inefficiencies?
3. **Security**: Are there potential vulnerabilities?
4. **Readability**: Is the code easy to understand?

## Tools

- `review_file`: Review a single file
- `suggest_fix`: Suggest improvements

## References

- [Review Checklist](../../../../echo-agent-learning/examples/demo_skills/code-review/references/checklist.md)
- [Style Guide](../../../../echo-agent-learning/examples/demo_skills/code-review/references/style_guide.md)
```

---

## 渐进式披露

文件型 Skill 采用三级渐进式披露，减少初始上下文占用：

```
┌─────────────────────────────────────────────────────────────────────┐
│                   Progressive Disclosure                             │
│                                                                      │
│   Level 1: Discovery (发现)                                         │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  Agent 启动时扫描 skills/ 目录                               │   │
│   │  为每个 SKILL.md 创建 SkillDescriptor                       │   │
│   │  注册 DiscoverySkillTool 用于后续激活                       │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Level 2: Activation (激活)                                        │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  LLM 调用 discovery_skill 激活某个 skill                     │   │
│   │  加载完整 SKILL.md 内容注入系统提示词                       │   │
│   │  注册该 skill 的所有工具                                     │   │
│   └─────────────────────────────────────────────────────────────┘   │
│                              │                                       │
│                              ▼                                       │
│   Level 3: Usage (使用)                                             │
│   ┌─────────────────────────────────────────────────────────────┐   │
│   │  LLM 调用具体工具完成任务                                    │   │
│   │  可访问 references/ 下的参考文档                             │   │
│   │  通过 hooks 拦截和增强工具调用                               │   │
│   └─────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
```

### 内置发现工具

```rust
// src/skills/external/discovery.rs

/// 发现可用技能
pub struct DiscoverySkillTool;

impl Tool for DiscoverySkillTool {
    fn name(&self) -> &str { "discovery_skill" }
    fn description(&self) -> &str { "发现并激活一个技能" }
    
    fn execute(&self, params: ToolParameters) -> BoxFuture<'_, Result<ToolResult>> {
        Box::pin(async move {
            let skill_name: String = params.get("name")?;
            // 激活技能
            // 注入提示词 + 注册工具
            Ok(ToolResult::success(format!("技能 {} 已激活", skill_name)))
        })
    }
}
```

---

## SkillRegistry

```rust
// src/skills/registry.rs
pub struct SkillRegistry {
    // Code-based skills
    skills: HashMap<String, Box<dyn Skill>>,
    
    // File-based skills (descriptors only)
    descriptors: Vec<SkillDescriptor>,
    
    // Hooks for tool call interception
    hooks: HookRegistry,
}

impl SkillRegistry {
    /// 注册代码型 Skill
    pub fn register(&mut self, skill: Box<dyn Skill>) {
        let info = SkillInfo {
            name: skill.name().to_string(),
            description: skill.description().to_string(),
            tool_names: skill.tools().iter().map(|t| t.name().to_string()).collect(),
            has_prompt_injection: skill.system_prompt_injection().is_some(),
        };
        self.skills.insert(info.name.clone(), skill);
    }
    
    /// 发现文件型 Skills
    pub fn discover(&mut self, path: &Path) -> Result<Vec<SkillDescriptor>> {
        for entry in fs::read_dir(path)? {
            let skill_path = entry?.path().join("SKILL.md");
            if skill_path.exists() {
                let descriptor = SkillDescriptor::from_file(&skill_path)?;
                self.descriptors.push(descriptor);
            }
        }
        Ok(self.descriptors.clone())
    }
    
    /// 激活文件型 Skill
    pub fn activate(&mut self, name: &str) -> Result<SkillContent> {
        let descriptor = self.descriptors.iter()
            .find(|d| d.name == name)
            .ok_or(SkillError::NotFound)?;
        
        let content = descriptor.load_content()?;
        // 注册工具、注入提示词
        Ok(content)
    }
}
```

---

## Hook 系统

Skill 可以定义 Hooks 来拦截和增强工具调用：

```rust
// src/skills/hooks.rs
pub struct HookRegistry {
    rules: Vec<HookRule>,
}

pub struct HookRule {
    pub skill_name: String,
    pub tool_pattern: Regex,
    pub event: HookEvent,
    pub action: HookAction,
}

pub enum HookEvent {
    BeforeCall,
    AfterCall,
    OnError,
}

pub enum HookAction {
    Transform(Box<dyn Fn(&mut Value) -> Result<()> + Send + Sync>),
    Validate(Box<dyn Fn(&Value) -> Result<()> + Send + Sync>),
    Log(Box<dyn Fn(&str, &Value) + Send + Sync>),
}
```

Skill 文件只使用官方字段。Hooks 由宿主应用配置或 Plugin Hook component 提供。

---

## 使用示例

### 代码型 Skill

```rust
use echo_agent::prelude::*;
use echo_agent::skills::Skill;

// 定义 Skill
struct GitWorkflowSkill;

impl Skill for GitWorkflowSkill {
    fn name(&self) -> &str { "git-workflow" }
    fn description(&self) -> &str { "Git 操作：分支、提交、PR/MR、冲突解决" }
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(GitStatusTool),
            Box::new(GitDiffTool),
        ]
    }
    fn system_prompt_injection(&self) -> Option<String> {
        Some("你能执行 git 工作流操作。修改前请先检查 status。".into())
    }
}

// 注册
let mut agent = ReactAgentBuilder::simple("qwen3-max", "助手")?;
agent.add_skill(Box::new(GitWorkflowSkill));
```

### 文件型 Skill

```rust
use echo_agent::skills::SkillRegistry;

// 发现
let mut registry = SkillRegistry::new();
let discovered = registry.discover(Path::new("skills/"))?;

println!("发现 {} 个技能:", discovered.len());
for desc in &discovered {
    println!("  - {}: {}", desc.name, desc.description);
}

// 在 Agent 中使用
agent.set_skill_registry(registry);

// LLM 可以调用 discovery_skill 激活需要的技能
```

---

## agentskills.io 规范

echo-agent 的 Skill 系统对齐 [agentskills.io](https://agentskills.io/specification) 规范：

| 规范要求 | echo-agent 实现 |
|----------|-----------------|
| SKILL.md 格式 | ✓ 支持 YAML frontmatter |
| 渐进式披露 | ✓ 三级：发现 → 激活 → 使用 |
| 工具注册 | ✓ 自动注册 |
| 提示词注入 | ✓ 系统提示词追加 |
| 参考文档 | ✓ references/ 目录 |
| Hooks | ✓ 拦截和增强 |

---

## 最佳实践

### 1. Skill 命名

- 使用小写字母、数字和连字符：`code-review`, `data-analyst`
- 命名应反映领域：`web-researcher` 而非 `research`
- 避免与内置 Skill 冲突

### 2. 提示词注入

```rust
fn system_prompt_injection(&self) -> Option<String> {
    Some(r#"
## 调研技能

你拥有网页调研能力。请使用以下工具：

- search: 用查询词搜索网页
- fetch: 下载特定 URL 的内容
- summarize: 对抓取页面做摘要

注意：优先使用权威来源；至少跨两个页面交叉验证关键论断。
"#.into())
}
```

### 3. 工具设计

- 每个 Skill 的工具应该相关且内聚
- 避免在一个 Skill 中混合不相关的工具
- 工具描述要清晰说明用途

### 4. 文件型 Skill 目录结构

```
skills/
└── my_skill/
    ├── SKILL.md              # 必需
    ├── references/           # 可选：参考文档
    │   ├── guide.md
    │   └── examples.md
    └── scripts/              # 可选：脚本工具
        └── helper.py
```

---

## 参考资料

- [agentskills.io 规范](https://agentskills.io/specification)
- [LangChain Tools](https://python.langchain.com/docs/modules/tools/)
- [CrewAI Tools](https://docs.crewai.com/core-concepts/Tools)
