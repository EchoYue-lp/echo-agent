package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class PromptCacheLayoutTest {
    private static JsonNode message(String role, String text) {
        return JsonSupport.MAPPER.createObjectNode()
                .put("role", role)
                .set("content", WireValues.value(text));
    }

    @Test
    void projectsRustCacheSegmentsAndRanges() {
        var layout = PromptCacheLayout.fromMessages(
                List.of(
                        message("system", "You are Echo Agent"),
                        message("system", "[Canonical context - restored]"),
                        message("user", "hello"),
                        message("assistant", "hi"),
                        message("user", "[runtime_context: turn 1]"),
                        message("user", "[runtime_context: hook]")),
                List.of(JsonSupport.MAPPER.createObjectNode().put("name", "lookup")));

        assertEquals(1, layout.system().size());
        assertEquals(1, layout.canonical().size());
        assertEquals(2, layout.history().size());
        assertEquals(2, layout.runtimeContext().size());
        assertEquals(1, layout.tools().size());
        var ranges = layout.segmentRanges();
        assertEquals(new SegmentRange(0L, 1L), ranges.system());
        assertEquals(new SegmentRange(1L, 2L), ranges.canonical());
        assertEquals(new SegmentRange(2L, 4L), ranges.history());
        assertEquals(new SegmentRange(4L, 6L), ranges.runtimeContext());
    }

    @Test
    void leavesCanonicalEmptyWithoutMarker() {
        var layout = PromptCacheLayout.fromMessages(
                List.of(message("system", "S"), message("user", "hello")), List.of());
        assertEquals(0, layout.canonical().size());
        assertEquals(1, layout.history().size());
        assertTrue(layout.segmentRanges().canonical().isEmpty());
    }
}
