package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;

class LlmApiProtocolTest {
    @Test
    void endpointHelpersPreserveRustProtocolDetection() {
        assertEquals("chat/completions", LlmApiProtocol.CHAT_COMPLETIONS.endpointPath());
        assertEquals(LlmApiProtocol.RESPONSES, LlmApiProtocol.fromEndpoint("https://api.openai.com/v1/responses?trace=true"));
        assertEquals(LlmApiProtocol.ANTHROPIC, LlmApiProtocol.fromEndpoint("https://api.anthropic.com/v1/messages"));
        assertEquals(LlmApiProtocol.CHAT_COMPLETIONS, LlmApiProtocol.fromEndpoint("https://gateway.example/v1/chat/completions"));
        assertNull(LlmApiProtocol.tryFromEndpoint("https://gateway.example/v1"));
        assertEquals(LlmApiProtocol.CHAT_COMPLETIONS, LlmApiProtocol.fromEndpoint("https://gateway.example/v1?upstream=https://api.anthropic.com/v1/"));
    }

    @Test
    void protocolMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean() && entry.path("languages").path("java").path("contract_test").asText().endsWith("/llm_api_protocol_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }
}
