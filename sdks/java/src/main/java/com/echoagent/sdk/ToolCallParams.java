package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.ObjectNode;

/** Type-safe view over a tool call's JSON parameter object. */
public final class ToolCallParams {
    private final JsonNode raw;
    private final ObjectNode values;

    private ToolCallParams(JsonNode raw) {
        this.raw = raw == null ? JsonSupport.MAPPER.createObjectNode() : raw.deepCopy();
        this.values = this.raw instanceof ObjectNode object
                ? object.deepCopy() : JsonSupport.MAPPER.createObjectNode();
    }

    public static ToolCallParams fromValue(JsonNode value) { return new ToolCallParams(value); }

    public static ToolCallParams fromParams(java.util.Map<String, ?> params) {
        if (params == null) throw new IllegalArgumentException("params must be a JSON object");
        var object = JsonSupport.MAPPER.createObjectNode();
        params.forEach((key, value) -> object.set(key, JsonSupport.MAPPER.valueToTree(value)));
        return new ToolCallParams(object);
    }

    public JsonNode raw() { return raw.deepCopy(); }

    public String getStr(String key) {
        JsonNode value = values.get(key);
        return value != null && value.isTextual() ? value.textValue() : null;
    }

    public Double getNumber(String key) {
        JsonNode value = values.get(key);
        return value != null && value.isNumber() ? value.doubleValue() : null;
    }

    public Boolean getBool(String key) {
        JsonNode value = values.get(key);
        return value != null && value.isBoolean() ? value.booleanValue() : null;
    }

    public JsonNode get(String key) {
        JsonNode value = values.get(key);
        return value == null ? null : value.deepCopy();
    }

    public void validateRequired(String key, String expectedType) {
        JsonNode value = values.get(key);
        if (value == null) throw new IllegalArgumentException("Missing required parameter: " + key);
        String actual = typeName(value);
        if (!actual.equals(expectedType)) {
            throw new IllegalArgumentException("Parameter '" + key + "': expected " + expectedType + ", got " + actual);
        }
    }

    public boolean has(String key) { return values.has(key); }
    public int len() { return values.size(); }
    public boolean isEmpty() { return values.isEmpty(); }

    private static String typeName(JsonNode value) {
        if (value.isNull()) return "null";
        if (value.isBoolean()) return "bool";
        if (value.isNumber()) return "number";
        if (value.isTextual()) return "string";
        if (value.isArray()) return "array";
        return "object";
    }
}
