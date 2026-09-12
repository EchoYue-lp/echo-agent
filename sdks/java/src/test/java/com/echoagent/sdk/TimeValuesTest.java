package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Instant;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class TimeValuesTest {
    @Test
    void timeHelpersPreserveInstantAndNullSemantics() {
        assertTrue(TimeValues.nowMillis() > 1_700_000_000_000L);
        assertTrue(TimeValues.nowSecs() > 1_700_000_000L);
        Instant source = Instant.parse("2026-07-09T01:50:48.876Z");
        String local = TimeValues.localRfc3339Serialize(source);
        assertTrue(local.matches(".*[+-][0-9]{2}:[0-9]{2}$"));
        assertEquals(source, TimeValues.localRfc3339Deserialize(local));
        assertEquals(source, TimeValues.optionLocalRfc3339Deserialize(local));
        assertNull(TimeValues.optionLocalRfc3339Serialize(null));
        assertNull(TimeValues.optionLocalRfc3339Deserialize(null));
        assertNotNull(TimeValues.nowLocal());
        assertThrows(IllegalArgumentException.class, () -> TimeValues.localRfc3339Deserialize("bad"));
    }

    @Test
    void timeHelperMappingsAreComplete() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(Path.of("../../contracts/sdk/parity-manifest.json")));
        int count = 0;
        for (JsonNode entry : manifest.path("entries")) {
            if (entry.path("canonical").asBoolean()
                    && entry.path("languages").path("java").path("contract_test")
                    .asText().endsWith("/time_values")) {
                count++;
                assertEquals("done", entry.path("languages").path("java").path("status").asText());
            }
        }
        assertEquals(8, count);
    }
}
