package com.echoagent.sdk;

import java.util.List;
import java.util.Locale;

/** Provider/model thinking profile projected without provider I/O. */
public record ThinkingProfile(ThinkingProtocol protocol, List<ThinkingLevel> levels) {
    public ThinkingProfile { levels = List.copyOf(levels); }
    public static ThinkingProfile newProfile(ThinkingProtocol protocol, List<ThinkingLevel> levels) { return new ThinkingProfile(protocol, levels); }
    public static ThinkingProfile unknown() { return new ThinkingProfile(ThinkingProtocol.NONE, List.of()); }
    public boolean supportsManualControl() { return protocol.emitsField() && !levels.isEmpty(); }

    public static ThinkingProfile resolveThinkingProfile(String provider, String modelName,
            String apiProtocol, String endpoint) {
        String providerLower = provider.trim().toLowerCase(Locale.ROOT);
        String model = modelName.trim().toLowerCase(Locale.ROOT);
        String endpointLower = endpoint == null ? "" : endpoint.toLowerCase(Locale.ROOT);
        boolean dashscope = List.of("dashscope", "qwen", "aliyun", "alibaba", "modelstudio", "bailian").contains(providerLower)
                || endpointLower.contains("dashscope.aliyuncs.com");
        boolean ollama = providerLower.equals("ollama") || endpointLower.contains("localhost:11434") || endpointLower.contains("127.0.0.1:11434");
        List<ThinkingLevel> toggle = List.of(ThinkingLevel.NONE, ThinkingLevel.HIGH);
        if (model.startsWith("claude-")) {
            int[] version = version(model, "claude-");
            if (version == null || version[0] < 4 || version[0] == 4 && version[1] < 6) return unknown();
            if (version[0] == 4 && version[1] == 6) return newProfile(
                    "anthropic".equals(apiProtocol) ? ThinkingProtocol.ANTHROPIC_EFFORT : ThinkingProtocol.OPENAI_REASONING_EFFORT,
                    List.of(ThinkingLevel.LOW, ThinkingLevel.MEDIUM, ThinkingLevel.HIGH, ThinkingLevel.XHIGH, ThinkingLevel.MAX));
            return newProfile(ThinkingProtocol.ANTHROPIC_ADAPTIVE, List.of());
        }
        if ("anthropic".equals(apiProtocol)) return unknown();
        if (ollama && "chat_completions".equals(apiProtocol)) {
            if (model.startsWith("gpt-oss")) return newProfile(ThinkingProtocol.OLLAMA_THINK, List.of(ThinkingLevel.LOW, ThinkingLevel.MEDIUM, ThinkingLevel.HIGH));
            if (List.of("qwen3", "deepseek-r1", "deepseek-v3", "deepseek-v4", "magistral").stream().anyMatch(model::startsWith)) return newProfile(ThinkingProtocol.OLLAMA_THINK, toggle);
            return unknown();
        }
        if (model.startsWith("gpt-5.6") || model.startsWith("gpt-5-6")) return newProfile(ThinkingProtocol.OPENAI_REASONING_EFFORT,
                List.of(ThinkingLevel.NONE, ThinkingLevel.LOW, ThinkingLevel.MEDIUM, ThinkingLevel.HIGH, ThinkingLevel.XHIGH, ThinkingLevel.MAX));
        if (model.startsWith("deepseek-v4")) return newProfile(dashscope && "chat_completions".equals(apiProtocol)
                ? ThinkingProtocol.ENABLE_THINKING_FLAG : ThinkingProtocol.DEEPSEEK_REASONING_EFFORT,
                dashscope && "chat_completions".equals(apiProtocol) ? toggle : List.of(ThinkingLevel.NONE, ThinkingLevel.LOW, ThinkingLevel.HIGH, ThinkingLevel.MAX));
        int[] glm = version(model, "glm-");
        if (glm != null && (glm[0] > 5 || glm[0] == 5 && glm[1] >= 2) && "chat_completions".equals(apiProtocol)) return newProfile(ThinkingProtocol.GLM_REASONING_EFFORT, List.of(ThinkingLevel.NONE, ThinkingLevel.HIGH, ThinkingLevel.MAX));
        if (model.startsWith("kimi-k3") && "chat_completions".equals(apiProtocol)) return newProfile(ThinkingProtocol.OPENAI_REASONING_EFFORT, List.of(ThinkingLevel.LOW, ThinkingLevel.HIGH, ThinkingLevel.MAX));
        if (model.startsWith("kimi-k2.7")) return newProfile(ThinkingProtocol.MODEL_MANAGED, List.of());
        if (model.startsWith("kimi-k2.6") && "chat_completions".equals(apiProtocol)) return newProfile(ThinkingProtocol.THINKING_TYPE, toggle);
        if (model.startsWith("qwen3") && "chat_completions".equals(apiProtocol)) return newProfile(ThinkingProtocol.ENABLE_THINKING_FLAG, toggle);
        if (model.startsWith("gemini-3") && "chat_completions".equals(apiProtocol)) return newProfile(ThinkingProtocol.OPENAI_REASONING_EFFORT, List.of(ThinkingLevel.MINIMAL, ThinkingLevel.LOW, ThinkingLevel.MEDIUM, ThinkingLevel.HIGH));
        if (model.startsWith("gemini-2.5") && "chat_completions".equals(apiProtocol)) return newProfile(ThinkingProtocol.OPENAI_REASONING_EFFORT, List.of(ThinkingLevel.NONE, ThinkingLevel.LOW, ThinkingLevel.MEDIUM, ThinkingLevel.HIGH));
        return unknown();
    }

    private static int[] version(String model, String prefix) {
        if (!model.startsWith(prefix)) return null;
        String[] segments = model.substring(prefix.length()).split("-");
        for (int index = 0; index < segments.length; index++) {
            String segment = segments[index];
            String[] parts = segment.split("\\.", -1);
            try {
                if (parts.length > 2) continue;
                int major = Integer.parseInt(parts[0]);
                int minor = parts.length > 1 ? Integer.parseInt(parts[1]) : 0;
                if (parts.length == 1 && index + 1 < segments.length) {
                    int candidate = Integer.parseInt(segments[index + 1]);
                    if (candidate >= 0 && candidate <= 9) minor = candidate;
                }
                if (major >= 3 && major <= 9) return new int[] {major, minor};
            } catch (NumberFormatException ignored) { }
        }
        return null;
    }
}
