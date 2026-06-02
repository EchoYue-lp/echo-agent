# Skill 创作指南

本指南教你如何为 echo-agent 创建自定义 Skill。Skill 系统的 API 和运行时机制参见 [07-skills.md](./07-skills.md)，本文聚焦于**如何编写高质量的 Skill 内容**。

---

## Skill 类型选择

| 类型 | 适用场景 | 复杂度 |
|------|---------|--------|
| **File-based** | 领域知识包、提示词 + 参考文件 + 脚本 | 低（只需 Markdown） |
| **Code-based** | 需要自定义工具逻辑、复杂计算 | 高（需写 Rust） |

**选择建议：** 只需领域知识和提示词 → File-based；需要自定义工具执行逻辑 → Code-based。

---

## File-based Skill（推荐入门）

对齐 [agentskills.io](https://agentskills.io/specification) 规范，无需修改代码。

### 目录结构

```
skills/
└── my-skill/
    ├── SKILL.md              # 必需：技能定义文件
    ├── references/           # 可选：参考文档
    │   ├── guide.md
    │   └── examples.md
    └── scripts/              # 可选：可执行脚本
        └── analyze.py
```

### SKILL.md 格式

```markdown
---
name: my-skill
description: 简短描述技能用途
version: 1.0.0
author: Your Name
tags: [domain, category]
activation_patterns:
  - "关键词1"
  - "关键词2"
---

# My Skill

## 何时使用
描述 Agent 应该在什么场景下激活这个技能。

## 使用指南
详细的使用说明和步骤。

## 可用资源
- `references/guide.md` - 详细指南

## 可用脚本
- `scripts/analyze.py` - 数据分析脚本
  - 参数：`--input <file>` `--output <file>`
```

### 三层渐进式披露

| 层级 | 内容 | 触发方式 | Token 开销 |
|------|------|---------|-----------|
| **Tier 1** | 名称 + 描述 | 启动时自动扫描 | ~50-100 / skill |
| **Tier 2** | 完整指引 + 资源列表 | `activate_skill` | <5000 / skill |
| **Tier 3** | 参考文件 / 脚本执行 | `read_skill_resource` / `run_skill_script` | 按需 |

**原则：** Tier 1 精简（只有 frontmatter），Tier 2 完整但不超 5000 tokens，Tier 3 按需加载。

### 编写高质量 SKILL.md

**✅ 好的写法：**
- 明确的 `activation_patterns`（避免太宽泛导致误激活）
- 结构化的使用指南（标题、列表、代码块）
- 提供输出格式示例
- 保持 Tier 2 < 5000 tokens

**❌ 避免：**
- 模糊的描述（如 `description: 代码相关功能`）
- 缺少具体步骤
- 超长的 SKILL.md（拆分到 references/）

### 编写脚本

脚本放在 `scripts/` 目录，支持 Python、Bash、Node.js 等：

```python
#!/usr/bin/env python3
"""数据分析脚本"""
import argparse
import pandas as pd

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--input', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    
    df = pd.read_csv(args.input)
    report = f"# 数据分析报告\n\n行数：{len(df)}\n"
    
    with open(args.output, 'w') as f:
        f.write(report)

if __name__ == '__main__':
    main()
```

在 SKILL.md 中引用：

```markdown
## 可用脚本
- `scripts/analyze.py` - 分析 CSV 数据
  - 参数：`--input <csv>` `--output <md>`
  - 依赖：`pandas`
```

### 测试

```bash
# 放置到项目级或用户级目录
mkdir -p ./skills/my-skill

# 验证加载
agent.discover_skills(vec!["./skills".into()]).await?;
```

---

## Code-based Skill

实现 `Skill` trait，适合需要自定义工具逻辑的场景：

```rust
use echo_agent::skills::Skill;
use echo_agent::tools::Tool;

pub struct MySkill;

impl Skill for MySkill {
    fn name(&self) -> &str { "my-skill" }
    fn description(&self) -> &str { "描述技能用途" }
    
    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![Box::new(MyTool1), Box::new(MyTool2)]
    }
    
    fn system_prompt_injection(&self) -> Option<String> {
        Some("当用户请求 X 时，先调用 my_tool1，再调用 my_tool2。".into())
    }
}

// 注册
agent.add_skill(Box::new(MySkill));
```

完整示例见 `demo07_skills.rs`。

---

## 最佳实践

| 原则 | 说明 |
|------|------|
| **命名** | Skill 用 `kebab-case`（`code-review`），工具用 `snake_case`（`web_search`） |
| **描述** | 具体明确：`"专业代码审查，识别缺陷和安全漏洞"` |
| **激活模式** | 精确：`["review code", "审查代码"]`，避免 `["code"]` |
| **提示词** | 精简（<200 字），明确行动指南 |
| **错误处理** | 验证输入、返回清晰错误信息 |

---

## 参考

- [Skill 系统 API](./07-skills.md) — SkillRegistry、DiscoveryScope、activate_skill
- [agentskills.io 规范](https://agentskills.io/specification)
- `demo07_skills.rs` — Code-based Skill 示例
- `demo08_external_skills.rs` — File-based Skill 示例
