use crate::skills::Skill;
use crate::tools::Tool;
use crate::tools::others::math::{AddTool, DivideTool, MultiplyTool, SubtractTool};

/// 计算器技能
///
/// 为 Agent 提供精确的四则运算能力，包括：
/// - `add`：两数相加
/// - `subtract`：两数相减  
/// - `multiply`：两数相乘
/// - `divide`：两数相除（含除零保护）
///
/// # 使用方式
/// ```rust
/// agent.add_skill(Box::new(CalculatorSkill));
/// ```
pub struct CalculatorSkill;

impl Skill for CalculatorSkill {
    fn name(&self) -> &str {
        "calculator"
    }

    fn description(&self) -> &str {
        "精确四则运算能力（加减乘除），避免 LLM 直接心算导致的精度误差"
    }

    fn tools(&self) -> Vec<Box<dyn Tool>> {
        vec![
            Box::new(AddTool),
            Box::new(SubtractTool),
            Box::new(MultiplyTool),
            Box::new(DivideTool),
        ]
    }

    fn system_prompt_injection(&self) -> Option<String> {
        Some(
            "\n\n## 计算器能力（Calculator Skill）\n\
             你拥有精确的数学计算工具，**禁止心算**，所有数值计算必须调用对应工具：\n\
             - `add(a, b)`：计算 a + b\n\
             - `subtract(a, b)`：计算 a - b\n\
             - `multiply(a, b)`：计算 a × b\n\
             - `divide(a, b)`：计算 a ÷ b（b 不能为 0）\n\
             对于多步骤计算，逐步调用工具，将上一步结果作为下一步输入。"
                .to_string(),
        )
    }
}
