# Skill 系统

## 是什么

Skill（技能）是比 Tool 更高层次的能力单元。一个 Skill 将一组相关的 Tool 和对应的 LLM 指引（系统提示词片段）打包在一起，作为一个整体"安装"到 Agent 上。

```
Tool:  单一原子操作（"读取文件"）
Skill: 领域能力包（"文件系统操作" = read_file + write_file + list_dir + 使用说明提示词）
```

---

## 解决什么问题

直接使用 Tool 的问题：
- **分散注册**：相关的 5 个工具需要分别调用 5 次 `add_tool()`
- **无语义引导**：工具有 description，但没有"何时用这组工具"的整体指引
- **复用困难**：同一组工具想用于不同 Agent，需要重复配置

Skill 将"工具集 + 使用方法"打包成可复用的能力单元，一次 `add_skill()` 解决所有问题。

---

## Skill vs Tool

| 维度 | Tool | Skill |
|------|------|-------|
| 粒度 | 单一操作 | 领域能力包 |
| 注册方式 | `agent.add_tool(box)` | `agent.add_skill(box)` |
| 系统提示词 | 无 | 可携带提示词注入片段 |
| 工具数量 | 1 个 | 多个 |
| 语义 | "做一件事" | "我掌握某个领域" |

---

## 内置 Skill

| Skill | 包含工具 | 描述 |
|-------|---------|------|
| `CalculatorSkill` | add/subtract/multiply/divide | 数学计算 |
| `FileSystemSkill` | read_file/write_file/list_dir | 文件系统操作 |
| `ShellSkill` | shell | Shell 命令执行 |
| `WeatherSkill` | get_weather | 天气查询 |

---

## 使用内置 Skill

```rust
use echo_agent::prelude::*;

let config = AgentConfig::new("qwen3-max", "assistant", "你是一个有帮助的助手")
    .enable_tool(true);

let mut agent = ReactAgent::new(config);

// 一次安装多个 Skill
agent.add_skill(Box::new(CalculatorSkill));
agent.add_skill(Box::new(FileSystemSkill));
// 等价于分别注册所有工具 + 在系统提示词末尾追加使用说明

let answer = agent.execute("计算 42 * 8，并将结果写入 result.txt").await?;
```

---

## 自定义 Skill

实现 `Skill` trait：

```rust
use echo_agent::skills::Skill;
use echo_agent::tools::{Tool, ToolParameters, ToolResult};
use echo_agent::error::Result;
use async_trait::async_trait;
use serde_json::{Value, json};

// 自定义工具
struct SearchTool;
struct SummarizeTool;

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str { "搜索网页内容" }
    fn parameters(&self) -> Value {
        json!({ "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] })
    }
    async fn execute(&self, _params: ToolParameters) -> Result<ToolResult> {
        Ok(ToolResult::success("搜索结果...".to_string()))
    }
}

// 省略 SummarizeTool 实现...

// 将工具打包为 Skill
struct ResearchSkill;

impl Skill for ResearchSkill {
    fn name(&self) -> &str { "research" }

    fn description(&self) -> &str { "网络研究能力：搜索 + 摘要" }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(SearchTool),
            Box::new(SummarizeTool),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some("当需要获取网络信息时，先用 web_search 搜索，再用 summarize 整理结果。\
              注意：搜索词要简洁，不要超过 5 个词。".to_string())
    }
}

// 安装到 Agent
let mut agent = ReactAgent::new(config);
agent.add_skill(Box::new(ResearchSkill));
```

---

## 外部 Skill（文件系统加载）

除了代码内定义的 Skill，还支持从目录加载 **SKILL.md** 文件定义的外部技能，无需修改代码即可扩展 Agent 能力。

### SKILL.md 格式

```markdown
---
name: code_review
description: 代码审查技能
tools:
  - read_file
  - shell
load_on_startup:
  - guidelines.md
---

## 使用说明

当用户要求审查代码时：
1. 使用 read_file 读取源文件
2. 对照 guidelines.md 中的规范检查
3. 输出结构化的审查意见
```

### 加载外部 Skill

```rust
// 扫描 skills/ 目录下的所有 SKILL.md
let loaded = agent.load_skills_from_dir("./skills").await?;
println!("已加载技能: {:?}", loaded);
```

### 目录结构示例

```
skills/
├── code_review/
│   ├── SKILL.md       ← 技能定义（YAML frontmatter + 指引文本）
│   └── guidelines.md  ← load_on_startup 资源（自动注入系统提示词）
└── data_analysis/
    ├── SKILL.md
    └── schema.json
```

---

## Skill 管理器

查询已安装的 Skill：

```rust
// 列出所有已安装 Skill
for info in agent.skill_manager().list() {
    println!(
        "- {} ({} 个工具, {}提示词注入)",
        info.name,
        info.tool_names.len(),
        if info.has_prompt_injection { "有" } else { "无" }
    );
}

// 检查某 Skill 是否已安装
if agent.skill_manager().is_installed("calculator") {
    println!("计算器技能已安装");
}
```

对应示例：`examples/demo07_skills.rs`、`examples/demo08_external_skills.rs`
