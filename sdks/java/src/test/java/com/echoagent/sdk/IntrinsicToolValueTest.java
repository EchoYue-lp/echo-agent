package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.LinkedHashMap;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class IntrinsicToolValueTest {
    @Test
    void toolCallParamsPreservesRustParameterTyping() {
        var params = ToolCallParams.fromParams(Map.of(
                "query", "echo", "limit", 3, "enabled", true,
                "nested", Map.of("key", "value")));
        assertEquals("echo", params.getStr("query"));
        assertEquals(3.0, params.getNumber("limit"));
        assertEquals(true, params.getBool("enabled"));
        assertEquals("value", params.get("nested").path("key").asText());
        assertFalse(params.has("missing"));
        assertEquals(4, params.len());
        assertFalse(params.isEmpty());
        params.validateRequired("query", "string");
        assertThrows(IllegalArgumentException.class,
                () -> params.validateRequired("query", "number"));
        assertThrows(IllegalArgumentException.class,
                () -> params.validateRequired("missing", "string"));
        assertTrue(ToolCallParams.fromValue(
                com.fasterxml.jackson.databind.node.TextNode.valueOf("value")).isEmpty());
    }

    @Test
    void toolResultConstructorsAndModifiersMatchRust() {
        var success = ToolResult.successJson(Map.of("answer", 42))
                .withMeta("source", "test")
                .withMimeType("application/json")
                .withTruncated(true)
                .toJson();
        assertEquals("json", success.path("kind").path("kind").asText());
        assertTrue(success.path("success").asBoolean());
        assertEquals("{\"answer\":42}", success.path("output").asText());
        assertEquals("map", success.path("data").path("kind").asText());
        assertEquals("test", success.path("metadata").path("source").asText());
        assertTrue(success.path("truncated").asBoolean());
        var collisionInput = new LinkedHashMap<String, Object>();
        collisionInput.put("kind", "string");
        collisionInput.put("value", "foo");
        var collision = ToolResult.successJson(collisionInput).toJson();
        assertEquals("map", collision.path("data").path("kind").asText());
        assertEquals(2, collision.path("data").path("value").size());
        assertEquals("kind", collision.path("data").path("value").get(0).path("key").path("value").asText());

        var invalid = ToolResult.invalidArguments("query required")
                .withOutput("bad input").toJson();
        assertFalse(invalid.path("success").asBoolean());
        assertEquals("query required", invalid.path("error").asText());
        assertEquals("invalid_arguments", invalid.path("failure").path("category").asText());
        assertEquals("correct_arguments", invalid.path("failure").path("recovery").asText());
        assertEquals("restore_then_retry", ToolResult.failure("unavailable", "offline")
                .toJson().path("failure").path("recovery").asText());
        assertEquals("verify_then_retry", ToolResult.failure("timeout", "slow")
                .toJson().path("failure").path("recovery").asText());
        assertEquals("verify_then_retry", ToolResult.failure("partial_side_effect", "partial")
                .toJson().path("failure").path("recovery").asText());
        assertEquals("retry", ToolResult.failure("transient", "retry")
                .toJson().path("failure").path("recovery").asText());
        assertEquals("stop", ToolResult.failure("permanent", "stop")
                .toJson().path("failure").path("recovery").asText());
        assertThrows(IllegalArgumentException.class, () -> ToolResult.failure("unknown", "bad"));
        var richFailure = JsonSupport.MAPPER.createObjectNode()
                .put("category", "transient")
                .put("recovery", "retry")
                .put("side_effect", "none")
                .put("retry_after_ms", "18446744073709551615")
                .put("idempotency_key", "key-1")
                .put("postcondition", "eventual success");
        assertTrue(ToolResult.success("ok").withFailure(richFailure).toJson().has("failure"));
        assertThrows(IllegalArgumentException.class, () -> ToolResult.success("ok").withFailure(
                richFailure.deepCopy().put("retry_after_ms", "01")));
        assertThrows(IllegalArgumentException.class, () -> ToolResult.success("ok").withFailure(
                richFailure.deepCopy().put("postcondition", 42)));
        assertTrue(ToolResult.success("ok").toJson().path("success").asBoolean());
        assertEquals("tool_error", ToolResult.error("failed").toJson()
                .path("kind").path("error_code").asText());
        assertEquals("image", ToolResult.successWithKind(
                JsonSupport.MAPPER.createObjectNode().put("kind", "image").put("mime_type", "image/png"),
                "image").toJson().path("kind").path("kind").asText());
        assertThrows(IllegalArgumentException.class, () -> ToolResult.successWithKind(
                JsonSupport.MAPPER.createObjectNode().put("kind", "image"), "image"));
        assertThrows(IllegalArgumentException.class, () -> ToolResult.successWithKind(
                JsonSupport.MAPPER.createObjectNode().put("kind", "unknown"), "bad"));
        assertEquals("value", ToolResult.success("ok").withMeta("", "value")
                .toJson().path("metadata").path("").asText());
        var content = ToolResult.success("ok")
                .withModelContent(JsonSupport.MAPPER.createObjectNode().put("url", "https://example.test/a"))
                .withModelContent(JsonSupport.MAPPER.createObjectNode().put("url", "https://example.test/b"))
                .toJson();
        assertEquals(2, content.path("model_content").size());
    }

    @Test
    void localToolValueRoutesHaveCompletedJavaMappings() throws Exception {
        Path manifestPath = Path.of("../..", "contracts/sdk/parity-manifest.json");
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(manifestPath));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && "intrinsic:language-local-wire-helper".equals(
                    entry.path("route").path("route").asText())) {
                count += 1;
                assertEquals("done", entry.path("languages").path("java").path("status").asText(),
                        entry.path("path").asText());
            }
        }
        assertEquals(28, count);
    }
}
