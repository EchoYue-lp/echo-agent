package com.echoagent.sdk;

import java.util.LinkedHashMap;
import java.util.Map;

/** Cumulative Subagent LLM usage values without owning provider execution. */
public final class LlmUsageStats {
    private String model = "";
    private long promptTokens;
    private long completionTokens;
    private long totalTokens;
    private long cachedPromptTokens;
    private long cacheCreationPromptTokens;
    private boolean usageReported;
    private long callCount;

    public void record(String model, long promptTokens, long completionTokens, long totalTokens,
            long cachedPromptTokens, long cacheCreationPromptTokens, boolean usageReported) {
        this.model = model;
        this.promptTokens += promptTokens;
        this.completionTokens += completionTokens;
        this.totalTokens += totalTokens;
        this.cachedPromptTokens += cachedPromptTokens;
        this.cacheCreationPromptTokens += cacheCreationPromptTokens;
        this.usageReported |= usageReported;
        this.callCount += 1;
    }

    public Map<String, Object> toPayload(String sessionId) {
        var payload = new LinkedHashMap<String, Object>();
        payload.put("session_id", sessionId);
        payload.put("model", model.isEmpty() ? "unknown" : model);
        payload.put("prompt_tokens", promptTokens);
        payload.put("completion_tokens", completionTokens);
        payload.put("total_tokens", totalTokens);
        payload.put("cached_prompt_tokens", cachedPromptTokens);
        payload.put("cache_creation_prompt_tokens", cacheCreationPromptTokens);
        payload.put("usage_reported", usageReported);
        payload.put("call_count", callCount);
        return payload;
    }
}
