package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/** Lossless builder for the wire {@code LlmChatResponse} value. */
public final class LlmChatResponse {
    private final ObjectNode json;

    private LlmChatResponse(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }
    public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private JsonNode message;
        private JsonNode raw;
        private JsonNode usage;
        private String finishReason;

        public Builder message(JsonNode value) { message = TypedExtensionSupport.copyObject(value, "message"); return this; }
        public Builder raw(Object value) { raw = TypedExtensionSupport.wireValue(value, "raw"); return this; }
        public Builder raw(JsonNode value) { raw = value == null ? null : value.deepCopy(); return this; }
        public Builder usage(JsonNode value) { usage = value == null ? null : value.deepCopy(); return this; }
        public Builder finishReason(String value) { finishReason = value; return this; }

        public LlmChatResponse build() {
            if (message == null || raw == null) throw new IllegalArgumentException("message and raw are required");
            var result = JsonSupport.MAPPER.createObjectNode();
            result.set("message", message.deepCopy());
            result.set("raw", raw.deepCopy());
            if (usage == null) result.putNull("usage"); else result.set("usage", usage.deepCopy());
            if (finishReason == null) result.putNull("finish_reason"); else result.put("finish_reason", finishReason);
            return new LlmChatResponse(result);
        }
    }
}
