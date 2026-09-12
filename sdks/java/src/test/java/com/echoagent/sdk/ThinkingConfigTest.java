package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

class ThinkingConfigTest {
    @Test
    void thinkingConfigPreservesProviderProjections() {
        assertEquals(ThinkingConfig.Kind.LEVEL, ThinkingConfig.medium().kind());
        assertNull(ThinkingConfig.parseSpec("auto"));
        assertEquals("budget_tokens", ThinkingConfig.parseSpec("4000").kind().name().toLowerCase());
        assertEquals("high", ThinkingConfig.parseSpec("high").toReasoningEffort());
        assertEquals("minimal", ThinkingConfig.disabled().toReasoningEffort());
        assertNull(ThinkingConfig.disabled().toAnthropicEffort());
        assertEquals(5_000, ThinkingConfig.medium().toAnthropicBudget(10_000));
        assertEquals(9_999, ThinkingConfig.budgetTokens(20_000).toAnthropicBudget(10_000));
        assertEquals("disabled", ThinkingConfig.level(ThinkingLevel.MINIMAL).toGlmThinkingType());
        assertEquals("enabled", ThinkingConfig.level(ThinkingLevel.HIGH).toGlmThinkingType());
        assertEquals("max", ThinkingConfig.budgetTokens(50_000).toGlmReasoningEffort());
        assertThrows(IllegalArgumentException.class, () -> ThinkingConfig.parseSpec("bogus"));
    }

    @Test
    void thinkingConfigMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/thinking_config_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(14, count);
    }
}
