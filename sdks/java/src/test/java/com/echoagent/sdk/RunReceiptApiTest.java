package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.util.concurrent.CompletionStage;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class RunReceiptApiTest {
    private static final String RECEIPT_STATUS =
            "echo_orchestration::runtime::turn_driver::TurnReceipt::status";
    private static final String OUTCOME_STATUS =
            "echo_orchestration::runtime::turn_driver::TurnOutcome::status";
    private static final String RECEIPT_USAGE =
            "echo_orchestration::runtime::turn_driver::TurnReceipt::usage";

    @Test
    void exposesReceiptQueriesAsCompletionStages() throws Exception {
        assertEquals(CompletionStage.class, RunHandle.class.getMethod("status").getReturnType());
        assertEquals(CompletionStage.class, RunHandle.class.getMethod("outcomeStatus").getReturnType());
        assertEquals(CompletionStage.class, RunHandle.class.getMethod("usage").getReturnType());
        assertEquals(CompletionStage.class, AgentHandle.class.getMethod("closeAsync").getReturnType());
    }

    @Test
    void canonicalCatalogResolvesReceiptQueriesAndWireU64StaysTextual() throws Exception {
        var catalog = new FacadeCatalog(
                java.nio.file.Path.of("../shared/facade-operation-catalog.json"),
                java.nio.file.Path.of("../shared/contract-digests.json"));
        assertEquals("_echo_agent/facade/invoke", catalog.resolve(RECEIPT_STATUS).method());
        assertEquals("_echo_agent/facade/invoke", catalog.resolve(OUTCOME_STATUS).method());
        assertEquals("_echo_agent/facade/invoke", catalog.resolve(RECEIPT_USAGE).method());

        JsonNode usage = JsonSupport.MAPPER.createObjectNode()
                .put("duration_ms", "18446744073709551615")
                .put("tokens_used", "9223372036854775808")
                .put("iterations", "0");
        assertTrue(usage.path("duration_ms").isTextual());
        assertTrue(usage.path("tokens_used").isTextual());
        assertTrue(usage.path("iterations").isTextual());
        assertEquals("18446744073709551615", usage.path("duration_ms").textValue());
        assertEquals("9223372036854775808", usage.path("tokens_used").textValue());
    }

    @Test
    void structuralBuildersPreserveTypedFieldsForHostClassification() {
        String typeId = "echo_sdk_protocol::methods::AgentEventWire";
        JsonNode event = WireValues.variant(typeId, "final_answer", Map.of("text", "done"));
        assertEquals("variant", event.path("kind").asText());
        assertEquals(typeId, event.path("value").path("type_id").asText());
        assertEquals("final_answer", event.path("value").path("variant").asText());
        assertEquals("string", event.path("value").path("fields").get(0).path("value").path("kind").asText());

        JsonNode record = WireValues.record(typeId, Map.of("event", "cancelled"));
        assertEquals("record", record.path("kind").asText());
        assertEquals("cancelled", record.path("value").path("fields").get(0).path("value").path("value").asText());
    }
}
