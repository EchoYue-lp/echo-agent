# Agent Factory、Mode Engine 与 Prompt Templates

## 概述

这三个组件协同工作，提供灵活的 Agent 创建和配置机制：

| 组件 | 职责 | 所在模块 |
|------|------|----------|
| Agent Factory | 根据执行范式创建 Agent | `echo-core::agent::factory` + `echo_agent::agent::default_factory` |
| Mode Engine | 按工作模式定制系统提示词和工具推荐 | `echo-core::agent::mode` + `echo_agent::agent::mode_engine` |
| Prompt Templates | 动态生成带变量替换的提示词 | `echo-core::agent::prompt_template` |

```
用户需求 → AgentFactory（选择范式）→ ModeEngine（选择模式）→ PromptTemplate（渲染提示词）→ Agent
```

---

## Agent Factory

### 是什么

Agent Factory 采用工厂模式，根据 `AgentParadigm`（执行范式）创建不同类型的 Agent。调用方无需了解具体的 Agent 实现类，只需提供配置即可获得对应范式的 Agent 实例。

### 执行范式（AgentParadigm）

| 范式 | 说明 | 适用场景 |
|------|------|----------|
| `React` | Think-Act-Observe 循环（默认） | 通用任务、对话、代码编写 |
| `PlanExecute` | 先规划步骤，再逐步执行 | 复杂多步骤任务、项目管理 |
| `SelfReflection` | 生成→批判→优化的迭代循环 | 需要高质量输出的创作、分析 |
| `Structured` | Schema 约束的结构化输出 | 需要确定性响应的数据提取、API 调用 |

### AgentFactoryConfig

`AgentFactoryConfig` 封装创建 Agent 所需的全部配置：

| 字段 | 类型 | 说明 |
|------|------|------|
| `paradigm` | `AgentParadigm` | 执行范式 |
| `mode` | `Option<AgentMode>` | 可选的工作模式（如 Coding、Research） |
| `model` | `String` | LLM 模型标识（如 "qwen3-max"） |
| `name` | `String` | Agent 名称（用于日志和编排） |
| `system_prompt` | `String` | 系统提示词 |
| `tools` | `Vec<Box<dyn Tool>>` | 自定义工具列表 |

提供便捷的构造函数：

```rust
AgentFactoryConfig::react()          // React 范式
AgentFactoryConfig::plan_execute()   // PlanExecute 范式
AgentFactoryConfig::self_reflection() // SelfReflection 范式
AgentFactoryConfig::structured()     // Structured 范式
```

### AgentFactory Trait

```rust
pub trait AgentFactory: Send + Sync {
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn Agent>>;
}
```

任何实现此 trait 的类型都可以作为 Agent 工厂。框架提供 `DefaultAgentFactory` 作为默认实现。

### DefaultAgentFactory

`DefaultAgentFactory` 是 facade 层（`echo_agent`）提供的具体工厂实现，基于 `ReactAgentBuilder` 构建 Agent：

| 范式 | Builder 配置 |
|------|-------------|
| React | `.enable_tools()` |
| PlanExecute | `.enable_tools().enable_planning()` |
| SelfReflection | `.enable_tools()`（反思循环为内置能力） |
| Structured | `.enable_tools()`（结构化输出单独配置） |

> **注意**：`echo-core` 中也定义了 `DefaultAgentFactory`，但它只是一个 stub，会返回错误。实际使用时必须使用 `echo_agent::agent::default_factory::DefaultAgentFactory`。

### 代码示例

```rust
use echo_agent::agent::default_factory::DefaultAgentFactory;
use echo_agent::agent::factory::{AgentFactoryConfig, AgentParadigm};
use echo_core::agent::factory::AgentFactory;

let factory = DefaultAgentFactory;

// 创建 React 范式的编程助手
let config = AgentFactoryConfig::react()
    .model("qwen3-max")
    .name("coder")
    .with_system_prompt("你是一个编程助手");

let agent = factory.create_agent(config)?;
println!("Agent: {}, Model: {}", agent.name(), agent.model_name());

// 创建 PlanExecute 范式的研究助手
let config = AgentFactoryConfig::plan_execute()
    .model("qwen3-max")
    .name("researcher")
    .with_mode(AgentMode::Research);

let agent = factory.create_agent(config)?;
```

---

## Mode Engine

### 是什么

Mode Engine 定义了 Agent 的工作模式（如 Coding、Research、Data Analysis、Writing），每种模式携带默认的系统提示词模板和推荐工具列表。应用层可以通过 ModeEngine 获取这些默认配置，并按需覆盖。

### AgentMode 枚举

| 模式 | 显示名称 | 图标 | 说明 |
|------|----------|------|------|
| `General` | General | 💬 | 通用助手，无特定领域专业化 |
| `Coding` | Coding | 💻 | 代码阅读、编写、调试、重构 |
| `Research` | Research | 🔬 | 学术论文搜索、分析、文献综述 |
| `Data` | Data Analysis | 📊 | 数据分析、统计、可视化 |
| `Writing` | Writing | ✍️ | 写作、编辑、文档格式化 |

### ModeConfig

`ModeEngine` 为每种模式返回的配置结构：

```rust
pub struct ModeConfig {
    pub system_prompt_template: String,  // 系统提示词模板
    pub recommended_tools: Vec<String>,    // 推荐工具名称（空表示不限制）
    pub display_name: String,            // 显示名称
    pub icon: String,                    // UI 图标/emoji
}
```

### ModeEngine Trait

```rust
pub trait ModeEngine: Send + Sync {
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig;
    fn all_modes(&self) -> Vec<AgentMode>;
    fn system_prompt(&self, mode: &AgentMode) -> String;
    fn recommended_tools(&self, mode: &AgentMode) -> Vec<String>;
}
```

### DefaultModeEngine

`DefaultModeEngine` 提供英文语言的默认提示词模板。每种模式的推荐工具列表：

| 模式 | 推荐工具数量 | 主要工具 |
|------|-------------|---------|
| General | 0（不限制） | 所有已注册工具 |
| Coding | 7 | shell, file_read, file_write, file_list, file_delete, code_search, git |
| Research | 8 | arxiv_search, semantic_scholar_search, pdf_fetch, bibtex_generate, web_search, web_fetch, file_read, file_write |
| Data | 16 | file_read, read_data, data_stats, profile_data, filter_data, aggregate_data, generate_chart, sample_data, correlate_data, pivot_data, time_series, hypothesis_test, regression, missing_value_analysis, outlier_detection, consistency_check |
| Writing | 4 | file_read, file_write, web_search, web_fetch |

### LocalizedModeEngine

`LocalizedModeEngine` 支持本地化提示词覆盖，回退到 `DefaultModeEngine` 获取未覆盖的模式配置：

```rust
use echo_agent::agent::mode_engine::LocalizedModeEngine;
use echo_core::agent::mode::{AgentMode, ModeEngine};

// 使用中文提示词
let engine = LocalizedModeEngine::with_chinese();
let config = engine.mode_config(&AgentMode::Coding);
println!("提示词: {}", config.system_prompt_template);  // 中文提示词
println!("显示名称: {}", config.display_name);           // "编程"
println!("推荐工具: {:?}", config.recommended_tools);    // 7 个工具

// 自定义覆盖
let engine = LocalizedModeEngine::new()
    .with_override(AgentMode::Coding, "自定义编程提示词".into())
    .with_display_name(AgentMode::Coding, "代码".into());

// 支持中英文名称解析
assert_eq!(LocalizedModeEngine::from_str("编程"), Some(AgentMode::Coding));
assert_eq!(LocalizedModeEngine::from_str("代码"), Some(AgentMode::Coding));
assert_eq!(LocalizedModeEngine::from_str("研究"), Some(AgentMode::Research));
assert_eq!(LocalizedModeEngine::from_str("coding"), Some(AgentMode::Coding));
```

### 模式解析

`AgentMode::from_name` 支持英文别名：

```rust
AgentMode::from_name("general")   // Some(AgentMode::General)
AgentMode::from_name("coding")    // Some(AgentMode::Coding)
AgentMode::from_name("code")      // Some(AgentMode::Coding)
AgentMode::from_name("research")  // Some(AgentMode::Research)
AgentMode::from_name("data")      // Some(AgentMode::Data)
AgentMode::from_name("writing")   // Some(AgentMode::Writing)
```

### 代码示例

```rust
use echo_core::agent::mode::{AgentMode, DefaultModeEngine, ModeEngine};

let engine = DefaultModeEngine;

// 获取编程模式的完整配置
let config = engine.mode_config(&AgentMode::Coding);
println!("模式: {} {}", config.icon, config.display_name);
println!("提示词: {}", config.system_prompt_template);
println!("推荐工具: {:?}", config.recommended_tools);

// 遍历所有模式
for mode in engine.all_modes() {
    let config = engine.mode_config(&mode);
    println!("{} {}: {} 个推荐工具",
        config.icon,
        config.display_name,
        config.recommended_tools.len()
    );
}
```

---

## Prompt Templates

### 是什么

`PromptTemplateManager` 是一个集中的提示词模板注册表和渲染引擎，支持变量替换、默认值和条件块。模板使用 `{{variable_name}}` 语法，线程安全（内部使用 `RwLock`）。

### 模板语法

| 语法 | 格式 | 说明 |
|------|------|------|
| 变量 | `{{name}}` | 用提供的值替换 |
| 默认值 | `{{name:default}}` | 未提供变量时使用默认值 |
| 条件块 | `{{#if var}}...{{#endif}}` | 变量存在且非空时包含块内容 |
| 条件+备选 | `{{#if var}}...{{#else}}...{{#endif}}` | 变量存在时显示第一部分，否则显示备选 |
| 嵌套条件 | `{{#if a}}{{#if b}}...{{#endif}}{{#endif}}` | 支持任意深度嵌套 |

### PromptTemplateManager API

| 方法 | 说明 |
|------|------|
| `new()` | 创建空模板管理器 |
| `with_default_mode_templates()` | 创建并预加载默认模式模板 |
| `register(name, template)` | 注册命名模板（覆盖同名） |
| `remove(name) -> bool` | 删除模板 |
| `contains(name) -> bool` | 检查模板是否存在 |
| `template_names() -> Vec<String>` | 列出所有模板名称 |
| `render(name, variables) -> Result<String>` | 按名称渲染模板 |
| `render_template(template, variables) -> String` | 直接渲染模板字符串 |
| `render_or_raw(name, variables) -> Result<String>` | 渲染或返回原始字符串（静态模板优化） |
| `get_template(name) -> Option<String>` | 获取原始模板字符串 |

### 代码示例

#### 基本变量替换

```rust
use echo_core::agent::prompt_template::PromptTemplateManager;

let manager = PromptTemplateManager::new();
manager.register("greeting", "你好，{{name}}！欢迎使用 {{project}}。");

let result = manager.render("greeting", &[
    ("name", "Alice"),
    ("project", "EchoAgent"),
]);
assert_eq!(result.unwrap(), "你好，Alice！欢迎使用 EchoAgent。");
```

#### 默认值

```rust
manager.register("fallback", "你好，{{name:访客}}！");

// 未提供 name 时使用默认值
let result = manager.render("fallback", &[]);
assert_eq!(result.unwrap(), "你好，访客！");

// 提供 name 时使用提供的值
let result = manager.render("fallback", &[("name", "Bob")]);
assert_eq!(result.unwrap(), "你好，Bob！");
```

#### 条件块

```rust
manager.register("detail",
    "基础信息。{{#if detail}}详情：{{detail}}。{{#endif}}结束。"
);

// 提供 detail 时显示
let result = manager.render("detail", &[("detail", "重要信息")]);
assert_eq!(result.unwrap(), "基础信息。详情：重要信息。结束。");

// 未提供时隐藏
let result = manager.render("detail", &[]);
assert_eq!(result.unwrap(), "基础信息。结束。");
```

#### 条件+备选

```rust
manager.register("level",
    "{{#if premium}}高级功能已启用。{{#else}}标准功能。{{#endif}}"
);

let result = manager.render("level", &[("premium", "true")]);
assert_eq!(result.unwrap(), "高级功能已启用。");

let result = manager.render("level", &[]);
assert_eq!(result.unwrap(), "标准功能。");
```

#### 嵌套条件

```rust
manager.register("nested",
    "{{#if a}}A 存在。{{#if b}}B 也存在。{{#else}}B 缺失。{{#endif}}{{#else}}A 缺失。{{#endif}}"
);

let result = manager.render("nested", &[("a", "yes"), ("b", "yes")]);
assert_eq!(result.unwrap(), "A 存在。B 也存在。");

let result = manager.render("nested", &[("a", "yes")]);
assert_eq!(result.unwrap(), "A 存在。B 缺失。");
```

#### 直接渲染（无需注册）

```rust
let manager = PromptTemplateManager::new();
let result = manager.render_template(
    "你好 {{who}}！",
    &[("who", "世界")]
);
assert_eq!(result, "你好 世界！");
```

#### 预加载模式模板

```rust
let manager = PromptTemplateManager::with_default_mode_templates();

// 自动注册了所有模式的模板
assert!(manager.contains("mode_general"));
assert!(manager.contains("mode_coding"));
assert!(manager.contains("mode_research"));
assert!(manager.contains("mode_data"));
assert!(manager.contains("mode_writing"));

// 渲染编程模式提示词
let prompt = manager.render("mode_coding", &[])?;
println!("{}", prompt);  // 输出完整的编程助手系统提示词
```

#### 线程安全共享

```rust
use std::sync::Arc;

let manager = Arc::new(PromptTemplateManager::new());
manager.register("shared", "你好，{{name}}！");

let m1 = Arc::clone(&manager);
let m2 = Arc::clone(&manager);

let r1 = m1.render("shared", &[("name", "A")]).unwrap();
let r2 = m2.render("shared", &[("name", "B")]).unwrap();

assert_eq!(r1, "你好，A！");
assert_eq!(r2, "你好，B！");
```

---

## 三组件协同

### 完整工作流

```
┌─────────────────────────────────────────────────────────────┐
│ 1. 选择范式 → AgentFactoryConfig::plan_execute()              │
│ 2. 选择模式 → .with_mode(AgentMode::Coding)                   │
│ 3. 选择模型 → .model("qwen3-max")                             │
│ 4. 创建 Agent → factory.create_agent(config)?                 │
│                                                             │
│    内部流程：                                                  │
│    ┌────────────────────────────────────────────────────┐   │
│    │ ModeEngine.mode_config(Coding)                      │   │
│    │   → 获取系统提示词模板                                │   │
│    │   → 获取推荐工具列表                                  │   │
│    │                                                     │   │
│    │ PromptTemplateManager.render("mode_coding", vars)   │   │
│    │   → 渲染提示词，替换变量                              │   │
│    │                                                     │   │
│    │ ReactAgentBuilder                                   │   │
│    │   → .system_prompt(rendered_prompt)                 │   │
│    │   → .tools(recommended_tools)                       │   │
│    │   → .enable_planning()  // PlanExecute 范式         │   │
│    │   → .build_boxed()                                  │   │
│    └────────────────────────────────────────────────────┘   │
│                                                             │
│ 5. 返回 Box<dyn Agent>                                       │
└─────────────────────────────────────────────────────────────┘
```

### 集成示例

```rust
use echo_agent::agent::default_factory::DefaultAgentFactory;
use echo_agent::agent::factory::{AgentFactory, AgentFactoryConfig};
use echo_agent::agent::mode_engine::LocalizedModeEngine;
use echo_core::agent::mode::{AgentMode, ModeEngine};
use echo_core::agent::prompt_template::PromptTemplateManager;

// 1. 初始化组件
let factory = DefaultAgentFactory;
let mode_engine = LocalizedModeEngine::with_chinese();
let template_manager = PromptTemplateManager::with_default_mode_templates();

// 2. 获取模式配置
let mode = AgentMode::Coding;
let mode_config = mode_engine.mode_config(&mode);

// 3. 使用模板管理器渲染提示词（可选，此处演示集成）
template_manager.register("custom_coding", &mode_config.system_prompt_template);
let system_prompt = template_manager.render("custom_coding", &[
    ("extra_instruction", "请优先使用 Rust 语言"),
])?;

// 4. 创建 Agent
let config = AgentFactoryConfig::plan_execute()
    .model("qwen3-max")
    .name("rust-coder")
    .with_mode(mode)
    .with_system_prompt(&mode_config.system_prompt_template);

let agent = factory.create_agent(config)?;
```

---

## 配置参考

### AgentParadigm 解析

| 输入字符串 | 解析结果 |
|-----------|---------|
| `"react"` | `AgentParadigm::React` |
| `"plan-execute"` / `"plan_execute"` | `AgentParadigm::PlanExecute` |
| `"self-reflection"` / `"self_reflection"` | `AgentParadigm::SelfReflection` |
| `"structured"` | `AgentParadigm::Structured` |

### AgentMode 解析（LocalizedModeEngine）

| 输入字符串 | 解析结果 |
|-----------|---------|
| `"general"` / `"通用"` | `AgentMode::General` |
| `"coding"` / `"code"` / `"编程"` / `"代码"` | `AgentMode::Coding` |
| `"research"` / `"研究"` | `AgentMode::Research` |
| `"data"` / `"数据分析"` / `"数据"` | `AgentMode::Data` |
| `"writing"` / `"写作"` / `"写"` | `AgentMode::Writing` |

### AgentFactoryConfig 默认值

| 字段 | 默认值 |
|------|--------|
| `paradigm` | `AgentParadigm::React` |
| `mode` | `None` |
| `model` | `""` |
| `name` | `"assistant"` |
| `system_prompt` | `"You are a helpful assistant"` |
| `tools` | `[]` |

### 模板语法速查

```
{{variable}}              → 变量替换
{{variable:default}}      → 带默认值的变量
{{#if var}}...{{#endif}}  → 条件块
{{#if var}}...{{#else}}...{{#endif}}  → 条件+备选
{{ name }}                → 空格自动去除
```

---

## 扩展指南

### 自定义 ModeEngine

```rust
use echo_core::agent::mode::{AgentMode, ModeConfig, ModeEngine};

pub struct MyCustomModeEngine;

impl ModeEngine for MyCustomModeEngine {
    fn mode_config(&self, mode: &AgentMode) -> ModeConfig {
        match mode {
            AgentMode::Coding => ModeConfig {
                system_prompt_template: "你是我的专属编程助手，遵循 PEP 8 风格。".into(),
                recommended_tools: vec!["shell".into(), "file_read".into()],
                display_name: "专属编程".into(),
                icon: "🛠️".into(),
            },
            // 其他模式可回退到 DefaultModeEngine
            _ => echo_core::agent::mode::DefaultModeEngine.mode_config(mode),
        }
    }
}
```

### 自定义 AgentFactory

```rust
use echo_core::agent::factory::{AgentFactory, AgentFactoryConfig};
use echo_agent::error::Result;
use echo_agent::agent::Agent;

pub struct MyAgentFactory;

impl AgentFactory for MyAgentFactory {
    fn create_agent(&self, config: AgentFactoryConfig) -> Result<Box<dyn Agent>> {
        // 根据 config.paradigm() 自定义创建逻辑
        // 可以注入自定义的 LLM 客户端、中间件等
        todo!("自定义 Agent 创建逻辑")
    }
}
```
