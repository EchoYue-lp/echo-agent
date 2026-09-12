package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;

class TurnModeTest {
    @Test
    void turnModesPreserveStableStreamFlavors() {
        assertEquals("chat", TurnMode.CHAT.asStr());
        assertEquals("execute", TurnMode.EXECUTE.asStr());
    }

    @Test
    void turnModeMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/turn_mode_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(3, count);
    }
}
