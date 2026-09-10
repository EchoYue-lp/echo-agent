package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.Set;

/** Typed reverse invocation for an Agent infrastructure component. */
public final class AgentComponentCall implements ExtensionCall {
    private static final Set<String> OPERATIONS = Set.of(
            "agent_component_call", "agent_component_call_stream");

    private final String operation;
    private final JsonNode input;
    private final AgentComponentRequest request;
    private final JsonNode context;
    private final JsonNode raw;

    private AgentComponentCall(String operation, JsonNode input, AgentComponentRequest request,
                               JsonNode context, JsonNode raw) {
        this.operation = operation;
        this.input = input;
        this.request = request;
        this.context = context;
        this.raw = raw;
    }

    public static AgentComponentCall from(JsonNode call) {
        var invocation = ToolCall.invocation(call);
        var operation = ToolCall.operation(invocation, OPERATIONS, "AgentComponent");
        var input = ToolCall.requiredObject(invocation, "input", "AgentComponent invocation input");
        if (!input.path("component").isTextual() || !input.path("call").isObject()) {
            throw new IllegalArgumentException("AgentComponent invocation input is malformed");
        }
        var request = AgentComponentRequest.from(input.path("call"));
        return new AgentComponentCall(operation, input, request,
                ToolCall.nullable(call, "context"), call.deepCopy());
    }

    @Override public String operation() { return operation; }
    @Override public JsonNode input() { return input.deepCopy(); }
    public String component() { return input.path("component").textValue(); }
    public String componentOperation() { return request.operation(); }
    public AgentComponentRequest request() { return request; }
    @Override public JsonNode context() { return context == null ? null : context.deepCopy(); }
    @Override public JsonNode raw() { return raw.deepCopy(); }
}
