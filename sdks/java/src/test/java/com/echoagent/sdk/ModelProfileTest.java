package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ModelProfileTest {
    @Test
    void modelProfilesPreserveRustProviderAndModelPolicy() {
        ModelProfile openai = ModelProfile.fromProviderName("gpt-5.6-sol", "openai");
        assertTrue(openai.supportsReasoning());
        assertEquals(16_384, openai.maxOutputTokens());
        assertEquals(1_050_000, openai.contextWindow());
        assertEquals("o200k_base", openai.tokenizerName());
        assertEquals(ThinkingProtocol.OPENAI_REASONING_EFFORT, openai.thinkingProtocol());
        assertTrue(openai.supportsImages());
        assertFalse(ModelProfile.fromProviderName("o3-mini", "openai").supportsImages());
        assertEquals(256_000, ModelProfile.inferContextWindow(" moonshot ", "kimi-k2.7-code"));
        assertEquals(null, ModelProfile.inferContextWindow("openai", "gpt-5.5"));
    }

    @Test
    void modelProfileResolverAppliesProviderThenExactOverrides() {
        ModelProfile profile = ModelProfileResolver.newResolver()
                .registerProviderDefault(" OpenAI ", new ModelProfileOverride(false, null, null, 99, Set.of("shell"), null))
                .registerExact("openai", "gpt-5.6-sol", new ModelProfileOverride(true, null, false, null, Set.of("browser"), "exact"))
                .resolve("openai", "gpt-5.6-sol", ProviderCapabilities.fromProviderName("openai"));
        assertTrue(profile.supportsParallelToolCalls());
        assertEquals(99, profile.contextWindow());
        assertFalse(profile.capabilities().structuredOutput());
        assertEquals("exact", profile.promptSuffix());
        assertEquals(Set.of("browser", "shell"), profile.excludedTools());
    }

    @Test
    void modelProfileMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean() && entry.path("languages").path("java").path("contract_test").asText().endsWith("/model_profile_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(32, count);
    }
}
