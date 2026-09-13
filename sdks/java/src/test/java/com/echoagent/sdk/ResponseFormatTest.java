package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.node.JsonNodeFactory;
import com.fasterxml.jackson.databind.node.ObjectNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ResponseFormatTest {
    @Test
    void responseFormatsPreserveRustTaggedValues() {
        assertFalse(ResponseFormat.text().isJson());
        assertTrue(ResponseFormat.jsonObject().isJson());
        ObjectNode schema = JsonNodeFactory.instance.objectNode().put("type", "object");
        ResponseFormat value = ResponseFormat.jsonSchema("answer", schema);
        assertTrue(value.isJson());
        assertEquals("json_schema", value.type());
        assertEquals("answer", value.jsonSchemaSpec().name());
        assertTrue(value.jsonSchemaSpec().strict());
        JsonNode copied = value.jsonSchemaSpec().schema();
        assertNotNull(copied);
        assertEquals("object", copied.path("type").asText());
    }

    @Test
    void responseFormatMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean() && entry.path("languages").path("java").path("contract_test").asText().endsWith("/response_format_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(6, count);
    }
}
