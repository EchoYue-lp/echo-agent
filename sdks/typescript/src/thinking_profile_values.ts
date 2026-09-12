import { ThinkingLevel } from "./thinking.js";
import { ThinkingProtocol, thinkingProtocolEmitsField } from "./thinking_protocol_values.js";

export type ThinkingProfile = Readonly<{
  protocol: ThinkingProtocol;
  levels: readonly ThinkingLevel[];
  supportsManualControl(): boolean;
}>;

const GPT_56 = [ThinkingLevel.None, ThinkingLevel.Low, ThinkingLevel.Medium, ThinkingLevel.High, ThinkingLevel.Xhigh, ThinkingLevel.Max] as const;
const CLAUDE_46 = [ThinkingLevel.Low, ThinkingLevel.Medium, ThinkingLevel.High, ThinkingLevel.Xhigh, ThinkingLevel.Max] as const;
const DEEPSEEK_V4 = [ThinkingLevel.None, ThinkingLevel.Low, ThinkingLevel.High, ThinkingLevel.Max] as const;
const GLM_52 = [ThinkingLevel.None, ThinkingLevel.High, ThinkingLevel.Max] as const;
const KIMI_K3 = [ThinkingLevel.Low, ThinkingLevel.High, ThinkingLevel.Max] as const;
const GEMINI_3 = [ThinkingLevel.Minimal, ThinkingLevel.Low, ThinkingLevel.Medium, ThinkingLevel.High] as const;
const GEMINI_25 = [ThinkingLevel.None, ThinkingLevel.Low, ThinkingLevel.Medium, ThinkingLevel.High] as const;
const TOGGLE = [ThinkingLevel.None, ThinkingLevel.High] as const;
const OLLAMA_GPT_OSS = [ThinkingLevel.Low, ThinkingLevel.Medium, ThinkingLevel.High] as const;

function profile(protocol: ThinkingProtocol, levels: readonly ThinkingLevel[]): ThinkingProfile {
  return Object.freeze({
    protocol,
    levels: Object.freeze([...levels]),
    supportsManualControl: () => thinkingProtocolEmitsField(protocol) && levels.length > 0,
  });
}

function version(model: string, prefix: string): [number, number] | undefined {
  const rest = model.startsWith(prefix) ? model.slice(prefix.length) : "";
  const segments = rest.split("-");
  for (let index = 0; index < segments.length; index += 1) {
    const segment = segments[index] ?? "";
    const match = /^(\d+)(?:\.(\d+))?$/u.exec(segment);
    if (!match) continue;
    const major = Number(match[1]);
    const next = segments[index + 1];
    const hyphenMinor = match[2] === undefined && next !== undefined && /^\d+$/u.test(next) && Number(next) <= 9 ? next : "0";
    const minor = Number(match[2] ?? hyphenMinor);
    if (major >= 3 && major <= 9) return [major, minor];
  }
  return undefined;
}

/** Resolve the provider/model thinking dialect without contacting a provider. */
export function resolveThinkingProfile(provider: string, modelName: string, apiProtocol = "chat_completions", endpoint?: string): ThinkingProfile {
  const providerLower = provider.trim().toLowerCase();
  const model = modelName.trim().toLowerCase();
  const endpointLower = (endpoint ?? "").toLowerCase();
  const dashscope = ["dashscope", "qwen", "aliyun", "alibaba", "modelstudio", "bailian"].includes(providerLower) || endpointLower.includes("dashscope.aliyuncs.com");
  const ollama = providerLower === "ollama" || endpointLower.includes("localhost:11434") || endpointLower.includes("127.0.0.1:11434");
  if (model.startsWith("claude-")) {
    const parsed = version(model, "claude-");
    if (!parsed || parsed[0] < 4 || parsed[0] === 4 && parsed[1] < 6) return profile(ThinkingProtocol.None, []);
    if (parsed[0] === 4 && parsed[1] === 6) return profile(apiProtocol === "anthropic" ? ThinkingProtocol.AnthropicEffort : ThinkingProtocol.OpenaiReasoningEffort, CLAUDE_46);
    return profile(ThinkingProtocol.AnthropicAdaptive, []);
  }
  if (apiProtocol === "anthropic") return profile(ThinkingProtocol.None, []);
  if (ollama && apiProtocol === "chat_completions") {
    if (model.startsWith("gpt-oss")) return profile(ThinkingProtocol.OllamaThink, OLLAMA_GPT_OSS);
    if (["qwen3", "deepseek-r1", "deepseek-v3", "deepseek-v4", "magistral"].some((prefix) => model.startsWith(prefix))) return profile(ThinkingProtocol.OllamaThink, TOGGLE);
    return profile(ThinkingProtocol.None, []);
  }
  if (model.startsWith("gpt-5.6") || model.startsWith("gpt-5-6")) return profile(ThinkingProtocol.OpenaiReasoningEffort, GPT_56);
  if (model.startsWith("deepseek-v4")) return profile(dashscope && apiProtocol === "chat_completions" ? ThinkingProtocol.EnableThinkingFlag : ThinkingProtocol.DeepseekReasoningEffort, dashscope && apiProtocol === "chat_completions" ? TOGGLE : DEEPSEEK_V4);
  const glm = version(model, "glm-");
  if (glm && (glm[0] > 5 || glm[0] === 5 && glm[1] >= 2) && apiProtocol === "chat_completions") return profile(ThinkingProtocol.GlmReasoningEffort, GLM_52);
  if (model.startsWith("kimi-k3") && apiProtocol === "chat_completions") return profile(ThinkingProtocol.OpenaiReasoningEffort, KIMI_K3);
  if (model.startsWith("kimi-k2.7")) return profile(ThinkingProtocol.ModelManaged, []);
  if (model.startsWith("kimi-k2.6") && apiProtocol === "chat_completions") return profile(ThinkingProtocol.ThinkingType, TOGGLE);
  if (model.startsWith("qwen3") && apiProtocol === "chat_completions") return profile(ThinkingProtocol.EnableThinkingFlag, TOGGLE);
  if (model.startsWith("gemini-3") && apiProtocol === "chat_completions") return profile(ThinkingProtocol.OpenaiReasoningEffort, GEMINI_3);
  if (model.startsWith("gemini-2.5") && apiProtocol === "chat_completions") return profile(ThinkingProtocol.OpenaiReasoningEffort, GEMINI_25);
  return profile(ThinkingProtocol.None, []);
}

export const ThinkingProfile = Object.freeze({
  new(protocol: ThinkingProtocol, levels: readonly ThinkingLevel[]): ThinkingProfile { return profile(protocol, levels); },
  unknown(): ThinkingProfile { return profile(ThinkingProtocol.None, []); },
});
