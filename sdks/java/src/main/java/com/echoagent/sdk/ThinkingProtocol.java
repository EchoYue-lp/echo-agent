package com.echoagent.sdk;

/** Provider thinking wire protocols projected as local values. */
public enum ThinkingProtocol {
    NONE("none"), MODEL_MANAGED("model_managed"),
    OPENAI_REASONING_EFFORT("openai_reasoning_effort"),
    DEEPSEEK_REASONING_EFFORT("deepseek_reasoning_effort"),
    ANTHROPIC_EFFORT("anthropic_effort"),
    ANTHROPIC_THINKING_BUDGET("anthropic_thinking_budget"),
    ANTHROPIC_ADAPTIVE("anthropic_adaptive"),
    ENABLE_THINKING_FLAG("enable_thinking_flag"), THINKING_TYPE("thinking_type"),
    GLM_REASONING_EFFORT("glm_reasoning_effort"), OLLAMA_THINK("ollama_think");

    private final String wireName;
    ThinkingProtocol(String wireName) { this.wireName = wireName; }
    public String asStr() { return wireName; }
    public boolean emitsField() {
        return this != NONE && this != MODEL_MANAGED && this != ANTHROPIC_ADAPTIVE;
    }
    @Override public String toString() { return wireName; }
}
