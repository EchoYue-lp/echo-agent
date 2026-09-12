package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

/** Common view of a Host-to-SDK reverse extension invocation. */
public interface ExtensionCall {
    String operation();
    JsonNode input();
    JsonNode context();
    JsonNode raw();

    /** Host-issued extension identity for this reverse invocation. */
    default WireHandle extension() {
        return WireHandle.fromJson(raw().path("extension"));
    }

    /** Domain invocation identity, independent from the JSON-RPC request id. */
    default String invocationId() {
        var value = raw().path("invocation_id");
        if (!value.isTextual() || value.textValue().isBlank()) {
            throw new IllegalArgumentException("invocation_id must be non-empty text");
        }
        return value.textValue();
    }

    /** Total reverse-call deadline in the lossless WireDuration shape. */
    default JsonNode deadline() {
        var value = raw().path("deadline");
        if (!value.isObject()) throw new IllegalArgumentException("deadline must be an object");
        return value.deepCopy();
    }

    /** Host-minted stream handle for streaming operations, or {@code null}. */
    default WireHandle stream() {
        var value = raw().get("stream");
        return value == null || value.isNull() ? null : WireHandle.fromJson(value);
    }
}
