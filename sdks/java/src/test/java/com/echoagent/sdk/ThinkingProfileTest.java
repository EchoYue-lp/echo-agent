package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ThinkingProfileTest {
    @Test
    void thinkingProfilesPreserveProviderModelSelection() {
        assertFalse(ThinkingProfile.unknown().supportsManualControl());
        assertEquals(ThinkingProtocol.OPENAI_REASONING_EFFORT, ThinkingProfile.resolveThinkingProfile("openai", "gpt-5.6-sol", "chat_completions", null).protocol());
        assertEquals(ThinkingProtocol.ANTHROPIC_EFFORT, ThinkingProfile.resolveThinkingProfile("anthropic", "claude-opus-4.6", "anthropic", null).protocol());
        assertEquals(ThinkingProtocol.ANTHROPIC_EFFORT, ThinkingProfile.resolveThinkingProfile("anthropic", "claude-opus-4-6", "anthropic", null).protocol());
        assertEquals(ThinkingProtocol.NONE, ThinkingProfile.resolveThinkingProfile("anthropic", "claude-opus-4.6.7", "anthropic", null).protocol());
        assertEquals(ThinkingProtocol.GLM_REASONING_EFFORT, ThinkingProfile.resolveThinkingProfile("zhipu", "glm-5-2", "chat_completions", null).protocol());
        assertEquals(ThinkingProtocol.OLLAMA_THINK, ThinkingProfile.resolveThinkingProfile("ollama", "qwen3-32b", "chat_completions", null).protocol());
        assertEquals(ThinkingProtocol.ENABLE_THINKING_FLAG, ThinkingProfile.resolveThinkingProfile("dashscope", "deepseek-v4-pro", "chat_completions", null).protocol());
        assertTrue(ThinkingProfile.newProfile(ThinkingProtocol.OPENAI_REASONING_EFFORT, List.of(ThinkingLevel.HIGH)).supportsManualControl());
    }

    @Test
    void thinkingProfileMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean() && entry.path("languages").path("java").path("contract_test").asText().endsWith("/thinking_profile_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(5, count);
    }
}
