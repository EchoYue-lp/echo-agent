package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ProviderCapabilitiesTest {
    @Test
    void providerCapabilitiesPreserveDefaultDialects() {
        assertTrue(ProviderCapabilities.fromProviderName("anthropic").namedSseEvents());
        assertFalse(ProviderCapabilities.fromProviderName(" anthropic ").namedSseEvents());
        assertTrue(ProviderCapabilities.fromProviderName("ollama").ndjsonStreaming());
        assertTrue(ProviderCapabilities.fromProviderName("custom").toolSupport());
        assertEquals("claude", ProviderCapabilities.anthropic().tokenizerName());
        assertTrue(ProviderCapabilities.openaiCompatible().supportsToolChoiceNone());
    }

    @Test
    void providerCapabilityMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean() && entry.path("languages").path("java").path("contract_test").asText().endsWith("/provider_capabilities_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(4, count);
    }
}
