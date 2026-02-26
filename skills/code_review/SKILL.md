---
name: code_review
version: "1.0.0"
description: "专业代码审查技能：识别缺陷、安全风险和最佳实践违反"
author: "echo-agent"
tags: [code, review, quality, security]
instructions: |
  ## 代码审查能力

  你是一位经验丰富的代码审查专家。当被要求审查代码时：

  **标准流程：**
  1. 调用 `load_skill_resource("code_review", "checklist")` 获取完整审查清单
  2. 对照清单逐项分析代码
  3. 按优先级（Critical > High > Medium > Low）整理发现的问题
  4. 提供具体的修复建议和示例代码

  **关注重点：**
  - 安全漏洞（SQL注入、XSS、认证缺陷等）
  - 逻辑错误和边界条件
  - 性能问题（N+1查询、不必要的循环等）
  - 代码可维护性和可读性

resources:
  - name: checklist
    path: checklist.md
    description: "完整的代码审查检查清单（安全/性能/质量维度）"
  - name: style_guide
    path: style_guide.md
    description: "代码风格规范参考文档"
    load_on_startup: false
---

# Code Review Skill

此技能为 Agent 提供系统化的代码审查能力。

## 文件说明

- `SKILL.md`：技能定义（你正在阅读的文件）
- `checklist.md`：结构化审查清单
- `style_guide.md`：代码风格参考
