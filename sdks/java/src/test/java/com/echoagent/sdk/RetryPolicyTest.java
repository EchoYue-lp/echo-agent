package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

class RetryPolicyTest {
    @Test
    void retryPolicyPreservesExponentialBackoff() {
        RetryPolicy policy = RetryPolicy.newPolicy(5, 100).maxDelay(800).jitter(false);
        assertEquals(0, policy.delayFor(0));
        assertEquals(100, policy.delayFor(1));
        assertEquals(200, policy.delayFor(2));
        assertEquals(800, policy.delayFor(4));
        assertEquals(0, RetryPolicy.noRetry().delayFor(1));
        assertThrows(IllegalArgumentException.class, () -> RetryPolicy.newPolicy(-1, 100));
    }

    @Test
    void retryPolicyMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/retry_policy_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(9, count);
    }
}
