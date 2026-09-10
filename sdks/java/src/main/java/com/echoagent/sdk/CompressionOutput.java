package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.util.Collection;
import java.util.List;

/** Lossless builder for the ContextCompressor output wire value. */
public final class CompressionOutput {
    private final ObjectNode json;

    private CompressionOutput(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }
    public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private List<JsonNode> messages = List.of();
        private List<JsonNode> evicted = List.of();
        private JsonNode checkpoint;

        public Builder messages(Collection<? extends JsonNode> value) {
            messages = snapshots(value, "messages");
            return this;
        }

        public Builder evicted(Collection<? extends JsonNode> value) {
            evicted = snapshots(value, "evicted");
            return this;
        }

        /** Sets the optional checkpoint as an already encoded WireValue. */
        public Builder checkpoint(JsonNode value) {
            checkpoint = value == null ? null : value.deepCopy();
            return this;
        }

        public CompressionOutput build() {
            var result = JsonSupport.MAPPER.createObjectNode();
            var encodedMessages = result.putArray("messages");
            messages.forEach(encodedMessages::add);
            var encodedEvicted = result.putArray("evicted");
            evicted.forEach(encodedEvicted::add);
            if (checkpoint == null) result.putNull("checkpoint");
            else result.set("checkpoint", checkpoint.deepCopy());
            return new CompressionOutput(result);
        }

        private static List<JsonNode> snapshots(
                Collection<? extends JsonNode> values, String name) {
            if (values == null) return List.of();
            var snapshots = new java.util.ArrayList<JsonNode>();
            for (JsonNode value : values) {
                if (value == null || !value.isObject()) {
                    throw new IllegalArgumentException(name + " entries must be LlmMessage objects");
                }
                snapshots.add(value.deepCopy());
            }
            return List.copyOf(snapshots);
        }
    }
}
