package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class HookEventTest {
    @Test
    void hookEventsPreserveNamesCategoriesAndMatchers() {
        assertEquals(31, HookEvent.ALL.size());
        assertEquals("PreToolUse", HookEvent.PRE_TOOL_USE.asStr());
        assertEquals(HookEvent.TASK_COMPLETED, HookEvent.fromName("TaskCompleted").orElseThrow());
        assertTrue(HookEvent.fromName("missing").isEmpty());
        assertEquals(HookEventCategory.TOOL, HookEvent.PRE_TOOL_USE.category());
        assertEquals(HookEventCategory.ERROR, HookEvent.STOP_FAILURE.category());
        assertEquals(HookEventCategory.EVOLUTION, HookEvent.RULE_PROMOTED.category());
        assertTrue(HookEvent.PERMISSION_DENIED.isToolEvent());
        assertFalse(HookEvent.SESSION_START.isToolEvent());
        assertTrue(HookEvent.STOP_FAILURE.supportsMatcher());
    }

    @Test
    void hookEventMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/hook_event_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(45, count);
    }
}
