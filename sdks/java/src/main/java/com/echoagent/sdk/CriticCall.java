package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.Set;

/** Typed reverse invocation received by a Critic implementation. */
public final class CriticCall implements ExtensionCall {
    private static final Set<String> OPERATIONS = Set.of("critic_critique");
    private static final int MAX_TEXT_CODE_POINTS = 65_536;

    private final String operation;
    private final JsonNode input;
    private final JsonNode context;
    private final JsonNode raw;

    private CriticCall(String operation, JsonNode input, JsonNode context, JsonNode raw) {
        this.operation = operation;
        this.input = input;
        this.context = context;
        this.raw = raw;
    }

    public static CriticCall from(JsonNode call) {
        var invocation = ToolCall.invocation(call);
        var operation = ToolCall.operation(invocation, OPERATIONS, "Critic");
        var input = ToolCall.requiredObject(invocation, "input", "Critic invocation input");
        requiredText(input, "task");
        requiredText(input, "answer");
        requiredText(input, "context");
        return new CriticCall(operation, input, ToolCall.nullable(call, "context"), call.deepCopy());
    }

    @Override public String operation() { return operation; }
    @Override public JsonNode input() { return input.deepCopy(); }
    public String task() { return requiredText(input, "task"); }
    public String answer() { return requiredText(input, "answer"); }
    /** Additional context supplied to the critic, distinct from envelope context metadata. */
    public String critiqueContext() { return requiredText(input, "context"); }
    /** Alias for callers that prefer the input field's name. */
    public String contextText() { return critiqueContext(); }
    @Override public JsonNode context() { return context == null ? null : context.deepCopy(); }
    @Override public JsonNode raw() { return raw.deepCopy(); }

    private static String requiredText(JsonNode object, String field) {
        var value = object.get(field);
        if (value == null || !value.isTextual()
                || value.textValue().codePointCount(0, value.textValue().length()) > MAX_TEXT_CODE_POINTS) {
            throw new IllegalArgumentException("Critic invocation input field " + field
                    + " must be bounded text");
        }
        return value.textValue();
    }
}
