package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ThinkingProtocolTest {
    @Test
    void thinkingProtocolsPreserveWireNamesAndEmission() {
        assertEquals("openai_reasoning_effort", ThinkingProtocol.OPENAI_REASONING_EFFORT.asStr());
        assertFalse(ThinkingProtocol.NONE.emitsField());
        assertFalse(ThinkingProtocol.MODEL_MANAGED.emitsField());
        assertFalse(ThinkingProtocol.ANTHROPIC_ADAPTIVE.emitsField());
        assertTrue(ThinkingProtocol.OLLAMA_THINK.emitsField());
    }

    @Test
    void thinkingProtocolMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/thinking_protocol_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(13, count);
    }
}
