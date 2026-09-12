package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;
import java.util.List;

/** Lossless builder for one Store item returned by a typed callback. */
public final class StoreItem {
    private final ObjectNode json;

    private StoreItem(ObjectNode json) { this.json = json; }

    public static Builder builder() { return new Builder(); }
    public ObjectNode toJson() { return json.deepCopy(); }

    public static final class Builder {
        private String key;
        private List<String> namespace = List.of();
        private JsonNode value;
        private BigInteger createdAt = BigInteger.ZERO;
        private BigInteger updatedAt = BigInteger.ZERO;
        private BigInteger expiresAt;
        private BigInteger lastAccessed;
        private float importance;
        private Float score;

        public Builder key(String value) { key = value; return this; }
        public Builder namespace(java.util.Collection<String> value) {
            namespace = List.copyOf(value == null ? List.of() : value);
            return this;
        }
        public Builder value(Object object) { value = TypedExtensionSupport.wireValue(object, "store item value"); return this; }
        public Builder value(JsonNode object) { value = object == null ? null : object.deepCopy(); return this; }
        public Builder createdAt(BigInteger value) { createdAt = value; return this; }
        public Builder updatedAt(BigInteger value) { updatedAt = value; return this; }
        public Builder expiresAt(BigInteger value) { expiresAt = value; return this; }
        public Builder lastAccessed(BigInteger value) { lastAccessed = value; return this; }
        public Builder importance(float value) { importance = value; return this; }
        public Builder score(Float value) { score = value; return this; }

        public StoreItem build() {
            var result = JsonSupport.MAPPER.createObjectNode();
            result.put("key", TypedExtensionSupport.requiredText(key, "key", 4096));
            result.set("namespace", TypedExtensionSupport.textArray(namespace, "namespace"));
            if (value == null) throw new IllegalArgumentException("store item value is required");
            result.set("value", value.deepCopy());
            result.put("created_at", TypedExtensionSupport.canonicalU64(createdAt, "createdAt"));
            result.put("updated_at", TypedExtensionSupport.canonicalU64(updatedAt, "updatedAt"));
            if (expiresAt == null) result.putNull("expires_at"); else result.put("expires_at", TypedExtensionSupport.canonicalU64(expiresAt, "expiresAt"));
            if (lastAccessed == null) result.putNull("last_accessed"); else result.put("last_accessed", TypedExtensionSupport.canonicalU64(lastAccessed, "lastAccessed"));
            result.put("importance", importance);
            if (score == null) result.putNull("score"); else result.put("score", score);
            return new StoreItem(result);
        }
    }
}
