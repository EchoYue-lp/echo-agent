package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class FacadeParityTest {
    private static Path repoPath(String relative) {
        return Path.of("../..", relative);
    }

    @Test
    void everyExternalSdkContractHasACompletedJavaMapping() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(repoPath("contracts/sdk/parity-manifest.json")));
        assertTrue(manifest.path("entries").isArray());
        assertFalse(manifest.path("entries").isEmpty());
        Set<String> expectedScopes = Set.of(
                "external_contract", "host_or_rust_only", "language_intrinsic", "internal_helper", "deferred");
        Map<String, Integer> counts = new HashMap<>();
        for (JsonNode entry : manifest.path("entries")) {
            JsonNode mapping = entry.path("languages").path("java");
            String scope = entry.path("sdk_scope").asText();
            assertTrue(expectedScopes.contains(scope), entry.path("path").asText());
            if (entry.path("canonical").asBoolean()) {
                counts.merge(scope, 1, Integer::sum);
            }
            if ("external_contract".equals(scope)) {
                assertEquals("done", mapping.path("status").asText(), entry.path("path").asText());
            }
            assertTrue(mapping.path("contract_test").asText().startsWith("sdk-parity/"));
        }
        assertEquals(Map.of(
                "external_contract", 5607,
                "host_or_rust_only", 1765,
                "language_intrinsic", 781,
                "internal_helper", 90,
                "deferred", 1441), counts);
    }

    @Test
    void catalogOperationIdentitiesRemainUniqueAndSigned() throws Exception {
        Path catalogPath = repoPath("sdks/shared/facade-operation-catalog.json");
        FacadeCatalog catalog = new FacadeCatalog(
                catalogPath,
                repoPath("sdks/shared/contract-digests.json"));
        Set<String> identities = new HashSet<>();
        for (FacadeCatalog.ResolvedOperation operation : catalog.operations()) {
            assertTrue(identities.add(operation.operation()), operation.operation());
            assertTrue(operation.signature().startsWith("sha256:"), operation.operation());
        }
        assertEquals(catalog.operations().size(), identities.size());
    }
}
