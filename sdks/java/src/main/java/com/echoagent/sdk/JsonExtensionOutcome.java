package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/** Immutable wire outcome shared by all typed outcome interfaces. */
final class JsonExtensionOutcome implements ToolOutcome, LlmOutcome, StoreOutcome, CriticOutcome, CompressionOutcome, AgentComponentOutcome {
    private final ObjectNode json;

    private JsonExtensionOutcome(ObjectNode json) { this.json = json; }

    static JsonExtensionOutcome result(String operation, JsonNode value) {
        if (operation == null || operation.isBlank() || value == null) {
            throw new IllegalArgumentException("result operation and value are required");
        }
        var result = JsonSupport.MAPPER.createObjectNode();
        result.put("operation", operation);
        result.set("value", value.deepCopy());
        return new JsonExtensionOutcome(JsonSupport.MAPPER.createObjectNode()
                .put("outcome", "result").set("result", result));
    }

    static JsonExtensionOutcome stream(WireHandle stream) {
        if (stream == null) throw new IllegalArgumentException("stream handle is required");
        return new JsonExtensionOutcome(JsonSupport.MAPPER.createObjectNode()
                .put("outcome", "stream").set("stream", stream.toJson()));
    }

    static JsonExtensionOutcome error(String code, String message, String retryable, JsonNode details) {
        var error = JsonSupport.MAPPER.createObjectNode()
                .put("code", TypedExtensionSupport.requiredText(code, "error code", 128))
                .put("message", TypedExtensionSupport.requiredText(message, "error message", 4096))
                .put("retryable", TypedExtensionSupport.requiredText(retryable, "retryable", 64));
        if (details == null) error.putNull("details");
        else error.set("details", details.deepCopy());
        return new JsonExtensionOutcome(JsonSupport.MAPPER.createObjectNode()
                .put("outcome", "error").set("error", error));
    }

    @Override public ObjectNode toJson() { return json.deepCopy(); }
}
