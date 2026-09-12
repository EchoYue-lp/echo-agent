package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.math.BigInteger;
import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ResourceLimitsTest {
    @Test
    void sandboxResourceLimitsPreservePolicies() {
        assertEquals(BigInteger.valueOf(30), ResourceLimits.defaults().cpuTimeSecs());
        assertEquals(BigInteger.valueOf(256).shiftLeft(20), ResourceLimits.defaults().memoryBytes());
        assertEquals(8, ResourceLimits.strict().maxProcesses());
        assertTrue(ResourceLimits.unrestricted().network());
    }

    @Test
    void sandboxResourceLimitMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/sandbox_resource_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(4, count);
    }
}
