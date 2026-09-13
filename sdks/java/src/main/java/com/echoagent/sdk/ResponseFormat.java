package com.echoagent.sdk;

/** Response format values; schema validation remains provider-owned. */
public final class ResponseFormat {
    private final String type;
    private final JsonSchemaSpec jsonSchema;

    private ResponseFormat(String type, JsonSchemaSpec jsonSchema) {
        this.type = type;
        this.jsonSchema = jsonSchema;
    }

    public static ResponseFormat text() { return new ResponseFormat("text", null); }
    public static ResponseFormat jsonObject() { return new ResponseFormat("json_object", null); }
    public static ResponseFormat jsonSchema(String name, com.fasterxml.jackson.databind.JsonNode schema) {
        return new ResponseFormat("json_schema", new JsonSchemaSpec(name, schema, true));
    }
    public String type() { return type; }
    public JsonSchemaSpec jsonSchemaSpec() { return jsonSchema; }
    public boolean isJson() { return "json_object".equals(type) || "json_schema".equals(type); }
}
