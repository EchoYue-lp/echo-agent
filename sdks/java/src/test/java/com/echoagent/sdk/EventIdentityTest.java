package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class EventIdentityTest {
    @Test
    void eventIdentitiesPreserveValidationAndImmutableUpdates() {
        assertEquals("evt-1", EventId.newId("evt-1").asStr());
        assertEquals("stream-1", StreamId.newId("stream-1").toString());
        assertThrows(IllegalArgumentException.class, () -> EventId.newId("  "));
        EventIdentity identity = EventIdentity.newIdentity("stream-1", "turn-1")
                .withRunId("run-1")
                .withMessageId("message-1")
                .withExecutionId("exec-1")
                .withConversationId("conversation-1")
                .withParentEventId("event-0");
        assertEquals("stream-1", identity.streamId().asStr());
        assertEquals("turn-1", identity.turnId());
        assertEquals("event-0", identity.parentEventId());
        assertEquals("run-2", EventIdentity.forRun("run-2").executionId());
        assertEquals("message-2", EventIdentity.forChat("conversation-2", "turn-2", "message-2", "run-2").messageId());
        assertEquals("exec-3", EventIdentity.fromRuntimeContext(Map.of("run_id", "run-3", "execution_id", "exec-3")).turnId());
    }

    @Test
    void eventIdentityMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/event_identity_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(29, count);
    }
}
