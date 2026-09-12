package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Collection;
import java.util.List;
import java.util.Map;
import java.util.Set;

/** Lossless builder for the wire {@code ToolResult} value. */
public final class ToolResult {
    private final ObjectNode json;

    private ToolResult(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }
    public static ToolResult success(String output) {
        return text(output, true).build();
    }

    public static ToolResult successJson(Object data) {
        try {
            var result = text(JsonSupport.MAPPER.writeValueAsString(data), true).kind("json").build();
            return result.withData(jsonToWire(data));
        } catch (com.fasterxml.jackson.core.JsonProcessingException error) {
            throw new IllegalArgumentException("tool result data is not JSON serializable", error);
        }
    }

    public static ToolResult successWithKind(String kind, String output) {
        return successWithKind(JsonSupport.MAPPER.createObjectNode().put("kind", kind), output);
    }

    public static ToolResult successWithKind(ObjectNode kind, String output) {
        validateKind(kind);
        var result = text(output, true).build();
        result.json.set("kind", kind.deepCopy());
        return result;
    }

    public static ToolResult error(String message) {
        var result = text("", false).build();
        result.json.set("kind", JsonSupport.MAPPER.createObjectNode()
                .put("kind", "structured_error").put("error_code", "tool_error"));
        return result
                .withError(message)
                .withFailure(failureJson("permanent", "stop", "none"));
    }

    public static ToolResult failure(String category, String message) {
        String recovery = recoveryFor(category);
        String sideEffect = "partial_side_effect".equals(category) ? "possible" : "none";
        return error(message).withFailure(failureJson(category, recovery, sideEffect));
    }

    public static ToolResult invalidArguments(String message) {
        return failure("invalid_arguments", message);
    }

    public static Builder text(String output, boolean success) {
        return builder().kind("text").output(output).success(success);
    }
    public ObjectNode toJson() { return json.deepCopy(); }

    public ToolResult withOutput(String output) {
        var result = copy();
        result.json.put("output", output == null ? "" : output);
        return result;
    }

    public ToolResult withError(String error) {
        var result = copy();
        result.json.put("success", false);
        if (error == null) result.json.putNull("error"); else result.json.put("error", error);
        if (!result.json.has("failure") || result.json.get("failure").isNull()) {
            result.json.set("failure", failureJson("permanent", "stop", "none"));
        }
        return result;
    }

    public ToolResult withFailure(JsonNode failure) {
        validateFailure(failure);
        var result = copy();
        result.json.put("success", false);
        result.json.set("failure", failure.deepCopy());
        return result;
    }

    public ToolResult withData(Object data) {
        return withData(jsonToWire(data));
    }

    public ToolResult withData(JsonNode data) {
        var result = copy();
        result.json.set("data", data == null ? com.fasterxml.jackson.databind.node.NullNode.instance : data.deepCopy());
        return result;
    }

    public ToolResult withTruncated(boolean truncated) {
        var result = copy();
        result.json.put("truncated", truncated);
        return result;
    }

    public ToolResult withMimeType(String mimeType) {
        var result = copy();
        if (mimeType == null) result.json.putNull("mime_type"); else result.json.put("mime_type", mimeType);
        return result;
    }

    public ToolResult withArtifact(JsonNode artifact) {
        var result = copy();
        result.json.set("artifact", artifact == null ? com.fasterxml.jackson.databind.node.NullNode.instance : artifact.deepCopy());
        return result;
    }

    public ToolResult withMeta(String key, String value) {
        if (key == null) throw new IllegalArgumentException("tool result metadata key must not be null");
        var result = copy();
        result.withMetadataObject().put(key, value == null ? "" : value);
        return result;
    }

    public ToolResult withMetadata(Map<String, String> metadata) {
        var result = copy();
        var object = result.withMetadataObject();
        object.removeAll();
        if (metadata != null) metadata.forEach((key, value) -> {
            if (key == null) throw new IllegalArgumentException("tool result metadata key must not be null");
            object.put(key, value == null ? "" : value);
        });
        return result;
    }

    public ToolResult withModelContents(Collection<JsonNode> content) {
        var result = copy();
        if (content != null) content.forEach(value -> result.appendModelContent(value));
        return result;
    }

    /** Append one provider-visible content item, matching Rust's by-value method. */
    public ToolResult withModelContent(JsonNode content) {
        var result = copy();
        result.appendModelContent(content);
        return result;
    }

    private ToolResult copy() { return new ToolResult(json.deepCopy()); }

    private ObjectNode withMetadataObject() {
        JsonNode existing = json.get("metadata");
        if (existing instanceof ObjectNode object) return object;
        return json.putObject("metadata");
    }

    private void appendModelContent(JsonNode content) {
        JsonNode existing = json.get("model_content");
        ArrayNode array = existing instanceof ArrayNode values ? values : json.putArray("model_content");
        array.add(content == null ? com.fasterxml.jackson.databind.node.NullNode.instance : content.deepCopy());
    }

    private static ObjectNode failureJson(String category, String recovery, String sideEffect) {
        return JsonSupport.MAPPER.createObjectNode()
                .put("category", category)
                .put("recovery", recovery)
                .put("side_effect", sideEffect);
    }

    private static String recoveryFor(String category) {
        if (category == null || !Set.of(
                "invalid_arguments", "unavailable", "timeout", "cancelled",
                "transient", "permanent", "partial_side_effect").contains(category)) {
            throw new IllegalArgumentException("unknown tool failure category: " + category);
        }
        return switch (category) {
            case "invalid_arguments" -> "correct_arguments";
            case "unavailable" -> "restore_then_retry";
            case "timeout", "partial_side_effect" -> "verify_then_retry";
            case "transient" -> "retry";
            default -> "stop";
        };
    }

    private static void validateFailure(JsonNode failure) {
        if (failure == null || !failure.isObject()) {
            throw new IllegalArgumentException("tool failure must be an object");
        }
        String category = failure.path("category").asText(null);
        String recovery = failure.path("recovery").asText(null);
        String sideEffect = failure.path("side_effect").asText(null);
        recoveryFor(category);
        if (!Set.of("correct_arguments", "retry", "restore_then_retry",
                "verify_then_retry", "stop").contains(recovery)) {
            throw new IllegalArgumentException("unknown tool recovery action: " + recovery);
        }
        if (!Set.of("none", "possible", "confirmed").contains(sideEffect)) {
            throw new IllegalArgumentException("unknown tool side effect: " + sideEffect);
        }
        JsonNode retryAfter = failure.get("retry_after_ms");
        if (retryAfter != null && !retryAfter.isNull()) {
            if (retryAfter.isTextual()) {
                WireValues.u64(retryAfter.textValue());
            } else if (retryAfter.isIntegralNumber()) {
                WireValues.u64(retryAfter.bigIntegerValue());
            } else {
                throw new IllegalArgumentException("tool failure retry_after_ms must be a u64");
            }
        }
        for (String field : List.of("idempotency_key", "postcondition")) {
            JsonNode value = failure.get(field);
            if (value != null && !value.isNull() && !value.isTextual()) {
                throw new IllegalArgumentException("tool failure " + field + " must be text");
            }
        }
    }

    private static void validateKind(ObjectNode kind) {
        if (kind == null || !kind.path("kind").isTextual()) {
            throw new IllegalArgumentException("tool result kind must contain a textual discriminator");
        }
        String discriminator = kind.path("kind").textValue();
        switch (discriminator) {
            case "text", "json" -> { }
            case "image" -> requireText(kind, "mime_type", discriminator);
            case "table" -> {
                if (!kind.path("columns").isArray() || !kind.path("rows").isArray()) {
                    throw new IllegalArgumentException("table result kind requires columns and rows");
                }
            }
            case "diff" -> requireText(kind, "unified_diff", discriminator);
            case "file_reference" -> requireText(kind, "path", discriminator);
            case "command_output" -> {
                JsonNode exitCode = kind.get("exit_code");
                if (exitCode != null && !exitCode.isNull() && !exitCode.isIntegralNumber()) {
                    throw new IllegalArgumentException("command_output result kind requires an integer exit_code");
                }
            }
            case "skill_activation" -> requireText(kind, "name", discriminator);
            case "structured_error" -> requireText(kind, "error_code", discriminator);
            default -> throw new IllegalArgumentException("unknown tool result kind: " + discriminator);
        }
    }

    private static void requireText(ObjectNode object, String field, String discriminator) {
        if (!object.path(field).isTextual()) {
            throw new IllegalArgumentException(discriminator + " result kind requires " + field);
        }
    }

    /** Encode ordinary JSON recursively as a WireValue map without kind/value auto-detection. */
    private static JsonNode jsonToWire(Object value) {
        JsonNode node = value instanceof JsonNode json
                ? json : JsonSupport.MAPPER.valueToTree(value);
        if (node == null || node.isNull()) return JsonSupport.MAPPER.createObjectNode().put("kind", "null");
        if (node.isBoolean()) return JsonSupport.MAPPER.createObjectNode().put("kind", "bool").put("value", node.booleanValue());
        if (node.isTextual()) return JsonSupport.MAPPER.createObjectNode().put("kind", "string").put("value", node.textValue());
        if (node.isIntegralNumber()) {
            return JsonSupport.wire(node.bigIntegerValue());
        }
        if (node.isFloatingPointNumber()) return JsonSupport.wire(node.doubleValue());
        if (node.isArray()) {
            var result = JsonSupport.MAPPER.createObjectNode().put("kind", "list");
            var values = result.putArray("value");
            node.forEach(item -> values.add(jsonToWire(item)));
            return result;
        }
        var result = JsonSupport.MAPPER.createObjectNode().put("kind", "map");
        var entries = result.putArray("value");
        node.fields().forEachRemaining(field -> {
            var pair = JsonSupport.MAPPER.createObjectNode();
            pair.set("key", JsonSupport.MAPPER.createObjectNode()
                    .put("kind", "string").put("value", field.getKey()));
            pair.set("value", jsonToWire(field.getValue()));
            entries.add(pair);
        });
        return result;
    }

    public static final class Builder {
        private String kind = "text";
        private String output;
        private boolean success;
        private boolean truncated;
        private String mimeType;
        private JsonNode data;
        private JsonNode artifact;
        private JsonNode failure;
        private Map<String, String> metadata = Map.of();
        private java.util.List<JsonNode> modelContent = java.util.List.of();

        public Builder kind(String value) {
            if (value == null || !java.util.List.of("text", "json").contains(value)) {
                throw new IllegalArgumentException("unknown tool result kind: " + value);
            }
            kind = value;
            return this;
        }
        public Builder output(String value) { output = value; return this; }
        public Builder success(boolean value) { success = value; return this; }
        public Builder truncated(boolean value) { truncated = value; return this; }
        public Builder mimeType(String value) { mimeType = value; return this; }
        public Builder data(Object value) { data = TypedExtensionSupport.wireValue(value, "tool result data"); return this; }
        public Builder data(JsonNode value) { data = value == null ? null : value.deepCopy(); return this; }
        public Builder artifact(JsonNode value) { artifact = value == null ? null : value.deepCopy(); return this; }
        public Builder failure(JsonNode value) { failure = value == null ? null : value.deepCopy(); return this; }
        public Builder metadata(Map<String, String> value) { metadata = Map.copyOf(value == null ? Map.of() : value); return this; }
        public Builder modelContent(java.util.Collection<JsonNode> value) {
            modelContent = java.util.List.copyOf(value == null ? java.util.List.of() : value);
            return this;
        }

        public ToolResult build() {
            var result = JsonSupport.MAPPER.createObjectNode();
            var kindValue = JsonSupport.MAPPER.createObjectNode().put("kind", kind);
            result.set("kind", kindValue);
            result.put("output", output == null ? "" : output);
            result.put("success", success);
            result.put("truncated", truncated);
            if (mimeType == null) result.putNull("mime_type"); else result.put("mime_type", mimeType);
            if (data == null) result.putNull("data"); else result.set("data", data.deepCopy());
            if (artifact == null) result.putNull("artifact"); else result.set("artifact", artifact.deepCopy());
            if (failure == null) result.putNull("failure"); else result.set("failure", failure.deepCopy());
            var metadataValue = result.putObject("metadata");
            metadata.forEach(metadataValue::put);
            ArrayNode content = result.putArray("model_content");
            modelContent.forEach(value -> content.add(value.deepCopy()));
            return new ToolResult(result);
        }
    }
}
