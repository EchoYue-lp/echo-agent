package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;

/** JSON Schema payload used by structured response formats. */
public record JsonSchemaSpec(String name, JsonNode schema, boolean strict) {
    public JsonSchemaSpec {
        schema = schema == null ? null : schema.deepCopy();
    }
}
