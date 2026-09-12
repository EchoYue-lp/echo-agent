/** Provider thinking wire protocols projected as local values. */
export const ThinkingProtocol = Object.freeze({
  None: "none",
  ModelManaged: "model_managed",
  OpenaiReasoningEffort: "openai_reasoning_effort",
  DeepseekReasoningEffort: "deepseek_reasoning_effort",
  AnthropicEffort: "anthropic_effort",
  AnthropicThinkingBudget: "anthropic_thinking_budget",
  AnthropicAdaptive: "anthropic_adaptive",
  EnableThinkingFlag: "enable_thinking_flag",
  ThinkingType: "thinking_type",
  GlmReasoningEffort: "glm_reasoning_effort",
  OllamaThink: "ollama_think",
} as const);

export type ThinkingProtocol = (typeof ThinkingProtocol)[keyof typeof ThinkingProtocol];

export function thinkingProtocolEmitsField(protocol: ThinkingProtocol): boolean {
  if (protocol === ThinkingProtocol.None || protocol === ThinkingProtocol.ModelManaged || protocol === ThinkingProtocol.AnthropicAdaptive) return false;
  return Object.values(ThinkingProtocol).includes(protocol);
}
