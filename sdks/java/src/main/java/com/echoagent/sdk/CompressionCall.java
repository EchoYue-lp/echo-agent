package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

import java.math.BigInteger;
import java.util.Set;

/** Typed reverse invocation received by a ContextCompressor implementation. */
public final class CompressionCall implements ExtensionCall {
    private static final Set<String> OPERATIONS = Set.of("compressor_compress");

    private final String operation;
    private final JsonNode input;
    private final JsonNode context;
    private final JsonNode raw;

    private CompressionCall(String operation, JsonNode input, JsonNode context, JsonNode raw) {
        this.operation = operation;
        this.input = input;
        this.context = context;
        this.raw = raw;
    }

    public static CompressionCall from(JsonNode call) {
        var invocation = ToolCall.invocation(call);
        var operation = ToolCall.operation(invocation, OPERATIONS, "ContextCompressor");
        var input = ToolCall.requiredObject(invocation, "input", "ContextCompressor invocation input");
        if (!input.path("messages").isArray()) {
            throw new IllegalArgumentException("ContextCompressor invocation input must contain messages");
        }
        for (JsonNode message : input.path("messages")) {
            if (!message.isObject()) {
                throw new IllegalArgumentException("ContextCompressor messages must be objects");
            }
        }
        var tokenLimit = input.get("token_limit");
        if (tokenLimit == null || !tokenLimit.isTextual()) {
            throw new IllegalArgumentException("ContextCompressor token_limit must be canonical u64 text");
        }
        TypedExtensionSupport.canonicalU64(tokenLimit.textValue(), "token_limit");
        optionalText(input, "current_query");
        optionalText(input, "focus_instructions");
        var tokenizer = input.path("tokenizer");
        if (!tokenizer.isObject() || !tokenizer.path("resource").isObject()) {
            throw new IllegalArgumentException("ContextCompressor tokenizer resource is malformed");
        }
        var resource = WireHandle.fromJson(tokenizer.path("resource"));
        if (!"facade_resource".equals(resource.kind())) {
            throw new IllegalArgumentException("ContextCompressor tokenizer must be a facade resource");
        }
        optionalText(tokenizer, "owner_session_id");
        if (!tokenizer.path("owner_session_id").isTextual()
                || tokenizer.path("owner_session_id").textValue().isBlank()) {
            throw new IllegalArgumentException("ContextCompressor tokenizer owner_session_id is required");
        }
        return new CompressionCall(operation, input, ToolCall.nullable(call, "context"), call.deepCopy());
    }

    @Override public String operation() { return operation; }
    @Override public JsonNode input() { return input.deepCopy(); }
    public JsonNode messages() { return input.path("messages").deepCopy(); }
    public BigInteger tokenLimit() { return new BigInteger(input.path("token_limit").textValue()); }
    public String currentQuery() { return optionalText(input, "current_query"); }
    public String focusInstructions() { return optionalText(input, "focus_instructions"); }
    public TokenizerReference tokenizer() {
        var tokenizer = input.path("tokenizer");
        return new TokenizerReference(
                WireHandle.fromJson(tokenizer.path("resource")),
                tokenizer.path("owner_session_id").textValue());
    }
    @Override public JsonNode context() { return context == null ? null : context.deepCopy(); }
    @Override public JsonNode raw() { return raw.deepCopy(); }

    private static String optionalText(JsonNode object, String field) {
        var value = object.get(field);
        if (value == null || value.isNull()) return null;
        if (!value.isTextual()) {
            throw new IllegalArgumentException(field + " must be text or null");
        }
        return value.textValue();
    }
}
