package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.math.BigInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;

class ExecutionUsageTest {
    @Test
    void executionUsageDurationDefaultsToZero() {
        assertEquals(BigInteger.ZERO, new ExecutionUsage(null, null, null).durationMillis());
        assertEquals(BigInteger.valueOf(42), new ExecutionUsage(BigInteger.valueOf(42), null, null).durationMillis());
    }

    @Test
    void executionUsageMappingIsComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/execution_usage_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(1, count);
    }
}
