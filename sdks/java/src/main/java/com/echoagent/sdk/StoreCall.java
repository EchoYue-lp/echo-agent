package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.util.List;
import java.util.Set;

/** Typed reverse invocation received by a Store implementation. */
public final class StoreCall implements ExtensionCall {
    private static final Set<String> OPERATIONS = Set.of(
            "store_put", "store_get", "store_search", "store_search_with", "store_delete",
            "store_list_namespaces", "store_list", "store_prune_expired", "store_dedup_by_content");
    private final String operation;
    private final JsonNode input;
    private final JsonNode context;
    private final JsonNode raw;

    private StoreCall(String operation, JsonNode input, JsonNode context, JsonNode raw) {
        this.operation = operation;
        this.input = input;
        this.context = context;
        this.raw = raw;
    }

    public static StoreCall from(JsonNode call) {
        var invocation = ToolCall.invocation(call);
        var operation = ToolCall.operation(invocation, OPERATIONS, "Store");
        var input = ToolCall.requiredObject(invocation, "input", "Store invocation input");
        return new StoreCall(operation, input, ToolCall.nullable(call, "context"), call.deepCopy());
    }

    @Override public String operation() { return operation; }
    @Override public JsonNode input() { return input.deepCopy(); }
    public String key() { return textOrNull("key"); }
    public List<String> namespace() { return TypedExtensionSupport.textList(input.get("namespace"), "namespace"); }
    public String query() { return textOrNull("query"); }
    public JsonNode value() { return input.get("value") == null ? null : input.get("value").deepCopy(); }
    private String textOrNull(String field) {
        var value = input.get(field);
        return value != null && value.isTextual() ? value.textValue() : null;
    }
    @Override public JsonNode context() { return context == null ? null : context.deepCopy(); }
    @Override public JsonNode raw() { return raw.deepCopy(); }
}
