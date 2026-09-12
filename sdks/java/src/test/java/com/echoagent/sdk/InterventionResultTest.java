package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class InterventionResultTest {
    @Test
    void interventionResultFactoriesPreserveLocalDecisions() {
        assertTrue(!InterventionResult.allow().block());
        assertEquals("reason", InterventionResult.block("reason").blockReason());
        assertEquals("context", InterventionResult.inject("context").injectedContext());
        assertTrue(InterventionResult.cancel().isCancelled());
        assertEquals("value", InterventionResult.modifyArgs(JsonSupport.MAPPER.createObjectNode()
                .put("key", "value")).modifiedArgs().path("key").asText());
    }

    @Test
    void interventionResultMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/intervention_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }
}
