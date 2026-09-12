package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;

class ModelInputModalityTest {
    @Test
    void modalityDefaultsPreserveRustOrderAndSpellings() {
        assertEquals("text", ModelInputModality.TEXT.asStr());
        assertEquals(List.of(ModelInputModality.TEXT), ModelInputModality.textOnly());
        assertEquals(List.of(ModelInputModality.TEXT, ModelInputModality.IMAGE, ModelInputModality.AUDIO, ModelInputModality.VIDEO), ModelInputModality.allSupported());
    }

    @Test
    void modalityMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean() && entry.path("languages").path("java").path("contract_test").asText().endsWith("/model_input_modality_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(7, count);
    }
}
