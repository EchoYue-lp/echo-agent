from __future__ import annotations

from enum import Enum


class ThinkingProtocol(str, Enum):
    NONE = "none"
    MODEL_MANAGED = "model_managed"
    OPENAI_REASONING_EFFORT = "openai_reasoning_effort"
    DEEPSEEK_REASONING_EFFORT = "deepseek_reasoning_effort"
    ANTHROPIC_EFFORT = "anthropic_effort"
    ANTHROPIC_THINKING_BUDGET = "anthropic_thinking_budget"
    ANTHROPIC_ADAPTIVE = "anthropic_adaptive"
    ENABLE_THINKING_FLAG = "enable_thinking_flag"
    THINKING_TYPE = "thinking_type"
    GLM_REASONING_EFFORT = "glm_reasoning_effort"
    OLLAMA_THINK = "ollama_think"

    def emits_field(self) -> bool:
        return self not in {
            ThinkingProtocol.NONE,
            ThinkingProtocol.MODEL_MANAGED,
            ThinkingProtocol.ANTHROPIC_ADAPTIVE,
        }
