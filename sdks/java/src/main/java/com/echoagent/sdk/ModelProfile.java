package com.echoagent.sdk;

import java.util.List;
import java.util.Locale;
import java.util.Set;

/** Rust-backed model policy projection; transport and execution remain host-owned. */
public record ModelProfile(String provider, String modelName, ProviderCapabilities capabilities,
        boolean supportsReasoning, ThinkingProtocol thinkingProtocol,
        List<ThinkingLevel> thinkingLevels, boolean supportsImages, boolean supportsTools,
        Integer maxOutputTokens, boolean supportsStreaming, boolean supportsParallelToolCalls,
        boolean supportsToolChoiceNone, Integer contextWindow, Set<String> excludedTools,
        String promptSuffix, String tokenizerName) {
    public ModelProfile {
        thinkingLevels = List.copyOf(thinkingLevels == null ? List.of() : thinkingLevels);
        excludedTools = Set.copyOf(excludedTools == null ? Set.of() : excludedTools);
    }

    public static ModelProfile newProfile(String modelName, String provider, ProviderCapabilities capabilities) {
        String lower = modelName.toLowerCase(Locale.ROOT);
        ThinkingProfile thinking = ThinkingProfile.resolveThinkingProfile(provider, modelName, "chat_completions", null);
        Integer maxOutputTokens = lower.contains("qwen3-235b") ? 131_072
                : (lower.startsWith("gpt-5") || lower.startsWith("o3") || lower.startsWith("o4")) ? 16_384
                : lower.startsWith("claude-") ? 8_192 : null;
        String tokenizerName = (lower.startsWith("gpt-5") || lower.startsWith("gpt-4.5")) ? "o200k_base"
                : (lower.startsWith("gpt-4") || lower.startsWith("gpt-3")) ? "cl100k_base"
                : capabilities.tokenizerName();
        return new ModelProfile(provider, modelName, capabilities,
                thinking.protocol() != ThinkingProtocol.NONE && capabilities.reasoningContent(),
                thinking.protocol(), thinking.levels(),
                capabilities.imageInput() && !lower.startsWith("o3-mini")
                        && !lower.startsWith("o1-mini") && !lower.startsWith("o1-preview"),
                capabilities.toolSupport(), maxOutputTokens, true,
                capabilities.supportsParallelToolCalls(), capabilities.supportsToolChoiceNone(),
                inferContextWindow(provider, modelName), Set.of(), null, tokenizerName);
    }

    public static ModelProfile fromProviderName(String modelName, String provider) {
        return newProfile(modelName, provider, ProviderCapabilities.fromProviderName(provider));
    }

    public static Integer inferContextWindow(String provider, String modelName) {
        String providerLower = provider.trim().toLowerCase(Locale.ROOT);
        String model = modelName.toLowerCase(Locale.ROOT);
        if ((providerLower.equals("openai") || providerLower.equals("azure-openai")) && model.startsWith("gpt-5.6")) return 1_050_000;
        if ((providerLower.equals("anthropic") && (model.startsWith("claude-fable-5") || model.startsWith("claude-opus-4-8") || model.startsWith("claude-sonnet-5")))
                || (providerLower.equals("deepseek") && model.startsWith("deepseek-v4"))
                || (List.of("dashscope", "qwen", "aliyun", "alibaba").contains(providerLower)
                        && (model.startsWith("qwen3.7-max") || model.startsWith("qwen3.7-plus")))
                || (providerLower.equals("zhipu") && model.startsWith("glm-5.2"))) return 1_000_000;
        if (providerLower.equals("moonshot") && (model.startsWith("kimi-k2.7") || model.startsWith("kimi-k2.6"))) return 256_000;
        return null;
    }
}
