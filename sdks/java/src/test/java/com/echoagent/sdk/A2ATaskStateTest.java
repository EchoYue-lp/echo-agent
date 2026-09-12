package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class A2ATaskStateTest {
    @Test
    void matchesRustTerminalAndTransitionTable() {
        assertFalse(TaskState.SUBMITTED.isTerminal());
        assertTrue(TaskState.COMPLETED.isTerminal());
        assertTrue(TaskState.FAILED.isTerminal());
        assertTrue(TaskState.CANCELED.isTerminal());
        assertTrue(TaskState.SUBMITTED.canTransitionTo(TaskState.WORKING));
        assertTrue(TaskState.WORKING.canTransitionTo(TaskState.INPUT_REQUIRED));
        assertTrue(TaskState.INPUT_REQUIRED.canTransitionTo(TaskState.WORKING));
        assertFalse(TaskState.COMPLETED.canTransitionTo(TaskState.WORKING));
        assertFalse(TaskState.SUBMITTED.canTransitionTo(TaskState.COMPLETED));
        assertEquals("input-required", TaskState.INPUT_REQUIRED.toString());
        assertThrows(IllegalArgumentException.class, () -> TaskState.SUBMITTED.canTransitionTo(null));
    }

    @Test
    void intrinsicIdentitiesHaveCompletedJavaMappings() throws Exception {
        Path manifestPath = Path.of("../..", "contracts/sdk/parity-manifest.json");
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(manifestPath));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/a2a_task_state")) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText(),
                        entry.path("path").asText());
            }
        }
        assertEquals(10, count);
    }
}
