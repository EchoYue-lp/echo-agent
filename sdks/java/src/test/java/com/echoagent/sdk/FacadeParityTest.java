package com.echoagent.sdk;

import com.fasterxml.jackson.databind.JsonNode;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

class FacadeParityTest {
    private static Path repoPath(String relative) {
        return Path.of("../..", relative);
    }

    @Test
    void everyExecutableFacadeItemHasACompletedJavaMapping() throws Exception {
        JsonNode manifest = JsonSupport.MAPPER.readTree(
                Files.readString(repoPath("contracts/sdk/parity-manifest.json")));
        assertTrue(manifest.path("entries").isArray());
        assertFalse(manifest.path("entries").isEmpty());
        for (JsonNode entry : manifest.path("entries")) {
            JsonNode mapping = entry.path("languages").path("java");
            if (!"intrinsic".equals(entry.path("route").path("surface").asText())) {
                assertEquals("done", mapping.path("status").asText(), entry.path("path").asText());
            }
            assertTrue(mapping.path("contract_test").asText().startsWith("sdk-parity/"));
        }
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
