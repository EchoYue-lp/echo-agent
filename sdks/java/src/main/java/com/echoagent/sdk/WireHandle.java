package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/** Opaque Host-issued handle. The generation is retained as a string so u64 values stay lossless. */
public record WireHandle(String id, String generation, String kind) {
    public WireHandle {
        if (id == null || id.isBlank() || id.codePointCount(0, id.length()) > 256
                || generation == null || kind == null || kind.isBlank()) {
            throw new IllegalArgumentException("handle id, generation and kind must be non-empty");
        }
        WireValues.u64(generation);
        if (!isKnownKind(kind)) {
            throw new IllegalArgumentException("unknown handle kind: " + kind);
        }
    }

    static WireHandle fromJson(JsonNode value) {
        if (value == null || !value.isObject()) {
            throw new IllegalArgumentException("handle must be an object");
        }
        return new WireHandle(
                required(value, "id"),
                required(value, "generation"),
                required(value, "kind"));
    }

    ObjectNode toJson() {
        var object = JsonSupport.MAPPER.createObjectNode();
        object.put("id", id);
        object.put("generation", generation);
        object.put("kind", kind);
        return object;
    }

    private static String required(JsonNode value, String field) {
        var node = value.get(field);
        if (node == null || !node.isTextual() || node.textValue().isBlank()) {
            throw new IllegalArgumentException("handle field " + field + " must be non-empty text");
        }
        return node.textValue();
    }

    private static boolean isKnownKind(String value) {
        return switch (value) {
            case "agent", "session", "run", "stream", "task_run", "plan_task",
                    "subagent", "extension", "facade_resource" -> true;
            default -> false;
        };
    }
}
