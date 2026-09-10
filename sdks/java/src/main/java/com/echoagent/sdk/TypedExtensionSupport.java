package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ArrayNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;
import java.util.Collection;
import java.util.List;
import java.util.Objects;

/** Package-private helpers shared by the typed extension facade. */
final class TypedExtensionSupport {
    private TypedExtensionSupport() {}

    static String requiredText(String value, String name, int maxCodePoints) {
        Objects.requireNonNull(value, name);
        if (value.isBlank() || value.codePointCount(0, value.length()) > maxCodePoints) {
            throw new IllegalArgumentException(name + " must be non-empty and bounded");
        }
        return value;
    }

    static String canonicalU64(BigInteger value, String name) {
        Objects.requireNonNull(value, name);
        if (value.signum() < 0 || value.compareTo(JsonSupport.MAX_U64) > 0) {
            throw new IllegalArgumentException(name + " is outside the u64 wire range");
        }
        return value.toString();
    }

    static String canonicalU64(String value, String name) {
        try {
            var normalized = canonicalU64(new BigInteger(value), name);
            if (!normalized.equals(value)) {
                throw new IllegalArgumentException(name + " must be canonical u64 text");
            }
            return normalized;
        } catch (RuntimeException error) {
            throw new IllegalArgumentException(name + " must be canonical u64 text", error);
        }
    }

    static ObjectNode baseDescriptor(String kind) {
        return JsonSupport.MAPPER.createObjectNode()
                .put("kind", kind)
                .put("descriptor_version", 1);
    }

    static ObjectNode copyObject(JsonNode value, String name) {
        if (value == null || !value.isObject()) {
            throw new IllegalArgumentException(name + " must be a JSON object");
        }
        return (ObjectNode) value.deepCopy();
    }

    static ObjectNode wireValue(Object value, String name) {
        try {
            return JsonSupport.wire(value);
        } catch (RuntimeException error) {
            throw new IllegalArgumentException(name + " is not a supported wire value", error);
        }
    }

    static ArrayNode textArray(Collection<String> values, String name) {
        var result = JsonSupport.MAPPER.createArrayNode();
        if (values == null) return result;
        for (String value : values) {
            result.add(requiredText(value, name + " entry", 256));
        }
        return result;
    }

    static List<String> textList(JsonNode value, String field) {
        if (value == null || !value.isArray()) return List.of();
        var result = new java.util.ArrayList<String>();
        for (JsonNode item : value) {
            if (!item.isTextual()) {
                throw new IllegalArgumentException(field + " entries must be text");
            }
            result.add(item.textValue());
        }
        return List.copyOf(result);
    }
}
