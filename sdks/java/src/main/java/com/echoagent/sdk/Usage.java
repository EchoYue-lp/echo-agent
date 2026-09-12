package com.echoagent.sdk;

/** Provider-normalized LLM usage values without owning provider execution. */
public record Usage(Long promptTokens, Long completionTokens, Long totalTokens,
        TokenUsageDetails promptTokensDetails, TokenUsageDetails inputTokensDetails,
        TokenUsageDetails outputTokensDetails, Long cacheCreationInputTokens,
        Long cacheReadInputTokens, Long promptCacheHitTokens, Long promptCacheMissTokens) {
    private static final long MAX_U32 = 0xffff_ffffL;
    public record TokenUsageDetails(Long cachedTokens, Long cacheWriteTokens, Long reasoningTokens) {}

    public Usage {
        validate(promptTokens, completionTokens, totalTokens, cacheCreationInputTokens,
                cacheReadInputTokens, promptCacheHitTokens, promptCacheMissTokens);
    }

    private static long saturating(long value) { return Math.min(MAX_U32, value); }

    public long cachedPromptTokens() {
        if (promptTokensDetails != null && promptTokensDetails.cachedTokens() != null) return promptTokensDetails.cachedTokens();
        if (inputTokensDetails != null && inputTokensDetails.cachedTokens() != null) return inputTokensDetails.cachedTokens();
        if (cacheReadInputTokens != null) return cacheReadInputTokens;
        if (promptCacheHitTokens != null) return promptCacheHitTokens;
        return 0L;
    }

    public long cacheCreationPromptTokens() {
        if (promptTokensDetails != null && promptTokensDetails.cacheWriteTokens() != null) return promptTokensDetails.cacheWriteTokens();
        if (inputTokensDetails != null && inputTokensDetails.cacheWriteTokens() != null) return inputTokensDetails.cacheWriteTokens();
        if (cacheCreationInputTokens != null) return cacheCreationInputTokens;
        return 0L;
    }

    public long effectivePromptTokens() {
        long prompt = promptTokens == null ? 0L : promptTokens;
        if (cacheReadInputTokens != null || cacheCreationInputTokens != null) return saturating(prompt + cachedPromptTokens() + cacheCreationPromptTokens());
        return prompt;
    }

    public long effectiveTotalTokens() {
        long completion = completionTokens == null ? 0L : completionTokens;
        if (cacheReadInputTokens != null || cacheCreationInputTokens != null) return saturating(effectivePromptTokens() + completion);
        return totalTokens == null ? saturating(effectivePromptTokens() + completion) : totalTokens;
    }

    public Double cacheHitRate() {
        long total = effectivePromptTokens();
        return total == 0L ? null : (double) cachedPromptTokens() / total;
    }

    private static void validate(Long... values) {
        for (Long value : values) {
            if (value != null && (value < 0 || value > MAX_U32)) {
                throw new IllegalArgumentException("usage token values must fit u32");
            }
        }
    }
}
