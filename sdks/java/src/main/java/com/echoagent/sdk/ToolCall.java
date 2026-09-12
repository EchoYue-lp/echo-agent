package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.Set;

/** Typed reverse invocation received by a Tool implementation. */
public final class ToolCall implements ExtensionCall {
    private static final Set<String> OPERATIONS = Set.of("tool_execute", "tool_execute_stream", "tool_validate_parameters");
    private final String operation;
    private final JsonNode input;
    private final JsonNode context;
    private final JsonNode raw;

    private ToolCall(String operation, JsonNode input, JsonNode context, JsonNode raw) {
        this.operation = operation;
        this.input = input;
        this.context = context;
        this.raw = raw;
    }

    public static ToolCall from(JsonNode call) {
        return parse(call);
    }

    static ToolCall parse(JsonNode call) {
        var invocation = invocation(call);
        var operation = operation(invocation, OPERATIONS, "Tool");
        var input = requiredObject(invocation, "input", "Tool invocation input");
        if (!input.has("parameters")) {
            throw new IllegalArgumentException("Tool invocation input must contain parameters");
        }
        return new ToolCall(operation, input, nullable(call, "context"), call.deepCopy());
    }

    @Override public String operation() { return operation; }
    @Override public JsonNode input() { return input.deepCopy(); }
    /** Parameters are the WireValue supplied to tool_execute/validate. */
    public JsonNode parameters() { return input.path("parameters").deepCopy(); }
    public JsonNode toolContext() { return nullable(input, "context"); }
    @Override public JsonNode context() { return context == null ? null : context.deepCopy(); }
    @Override public JsonNode raw() { return raw.deepCopy(); }

    static JsonNode invocation(JsonNode call) {
        return requiredObject(call, "invocation", "extension invocation");
    }

    static JsonNode requiredObject(JsonNode object, String field, String description) {
        if (object == null || !object.isObject() || !object.path(field).isObject()) {
            throw new IllegalArgumentException(description + " must be an object");
        }
        return object.path(field);
    }

    static String operation(JsonNode invocation, Set<String> allowed, String type) {
        var node = invocation.get("operation");
        if (node == null || !node.isTextual() || !allowed.contains(node.textValue())) {
            throw new IllegalArgumentException(type + " invocation has an unsupported operation");
        }
        return node.textValue();
    }

    static JsonNode nullable(JsonNode object, String field) {
        if (object == null || object.get(field) == null || object.get(field).isNull()) return null;
        return object.get(field).deepCopy();
    }
}
