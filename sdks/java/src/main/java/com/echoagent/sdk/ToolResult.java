package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Map;

/** Lossless builder for the wire {@code ToolResult} value. */
public final class ToolResult {
    private final ObjectNode json;

    private ToolResult(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }
    public static Builder text(String output, boolean success) {
        return builder().kind("text").output(output).success(success);
    }
    public ObjectNode toJson() { return json.deepCopy(); }

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
