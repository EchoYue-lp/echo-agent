package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

/** Stable SDK extension failure; callers must not parse ACP human messages for state. */
public final class EchoAgentException extends RuntimeException {
    private final String code;
    private final String retryable;
    private final String operation;
    private final JsonNode details;

    public EchoAgentException(String code, String message, String retryable, String operation, JsonNode details) {
        super(message);
        this.code = code;
        this.retryable = retryable;
        this.operation = operation;
        this.details = details;
    }

    public String code() { return code; }
    public String retryable() { return retryable; }
    public String operation() { return operation; }
    public JsonNode details() { return details; }

    static EchoAgentException from(JsonNode data, String operation) {
        var object = data != null && data.isObject() ? data : null;
        return new EchoAgentException(
                text(object, "code", "transport_error"),
                text(object, "message", "ACP request failed"),
                text(object, "retryable", "never"),
                object != null && object.has("operation") && object.get("operation").isTextual()
                        ? object.get("operation").textValue() : operation,
                object == null ? null : object.get("details"));
    }

    private static String text(JsonNode node, String field, String fallback) {
        return node != null && node.has(field) && node.get(field).isTextual()
                ? node.get(field).textValue() : fallback;
    }
}
