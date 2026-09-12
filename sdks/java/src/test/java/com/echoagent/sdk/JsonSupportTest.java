package com.echoagent.sdk;

import org.junit.jupiter.api.Test;

import java.math.BigInteger;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class JsonSupportTest {
    @Test
    void handleIsEncodedAsWireHandleBeforeGenericObjects() {
        var handle = new WireHandle("session-1", "1", "session");
        var encoded = JsonSupport.wire(handle);
        assertEquals("handle", encoded.path("kind").asText());
        assertEquals(handle.id(), encoded.path("value").path("id").asText());
    }

    @Test
    void u64RemainsTextual() {
        var encoded = JsonSupport.wire(new BigInteger("9223372036854775808"));
        assertEquals("u64", encoded.path("kind").asText());
        assertEquals("9223372036854775808", encoded.path("value").asText());
    }

    @Test
    void nestedMapsAndListsRemainStructured() {
        var input = Map.of("answer", List.of("a", Map.of("count", "18446744073709551615")));
        var decoded = JsonSupport.fromWire(JsonSupport.wire(input));
        assertEquals("a", decoded.path("answer").get(0).asText());
        assertEquals("18446744073709551615", decoded.path("answer").get(1).path("count").asText());
    }

    @Test
    void integerBoundsMatchWireContract() {
        assertEquals("-9223372036854775808", JsonSupport.wire(JsonSupport.MIN_I64).path("value").asText());
        assertEquals("18446744073709551615", JsonSupport.wire(JsonSupport.MAX_U64).path("value").asText());
        assertThrows(IllegalArgumentException.class,
                () -> JsonSupport.wire(JsonSupport.MAX_U64.add(BigInteger.ONE)));
        assertThrows(IllegalArgumentException.class,
                () -> JsonSupport.wire(JsonSupport.MIN_I64.subtract(BigInteger.ONE)));
    }

    @Test
    void canonicalResolverSelectsSourceAndFamilyRoutes() throws Exception {
        var catalog = new FacadeCatalog(
                Path.of("../shared/facade-operation-catalog.json"),
                Path.of("../shared/contract-digests.json"));
        assertEquals("_echo_agent/facade/invoke", catalog.resolve("echo_core::agent::Agent::name").method());
        assertEquals("_echo_agent/memory/op", catalog.resolve("memory.store.put").method());
    }

    @Test
    void canonicalCatalogEnumeratesEverySourceAndFamilyOperationOnce() throws Exception {
        var catalogPath = Path.of("../shared/facade-operation-catalog.json");
        var catalog = new FacadeCatalog(
                catalogPath,
                Path.of("../shared/contract-digests.json"));

        var document = JsonSupport.MAPPER.readTree(java.nio.file.Files.readString(catalogPath));
        long expectedOperations = 0;
        for (var route : document.path("routes")) {
            if (route.path("operation").isTextual()) expectedOperations++;
            expectedOperations += route.path("operation_signatures").size();
        }
        assertEquals(expectedOperations, catalog.operations().size());
        assertEquals(document.path("families").size(), catalog.families().size());

        Set<String> identities = new HashSet<>();
        String previousOperation = "";
        for (var operation : catalog.operations()) {
            assertTrue(operation.operation().compareTo(previousOperation) >= 0);
            previousOperation = operation.operation();
            assertTrue(identities.add(operation.operation()), operation.operation());
            assertTrue(operation.family() != null && !operation.family().isBlank());
            assertTrue(operation.method() != null && !operation.method().isBlank());
            assertTrue(operation.signature().startsWith("sha256:"));
        }

        Set<String> familyIdentities = new HashSet<>();
        for (var family : catalog.families()) {
            for (var operation : family.operations()) {
                assertEquals(family.family(), operation.family());
                assertTrue(family.methods().isEmpty() || family.methods().contains(operation.method()));
                assertTrue(familyIdentities.add(operation.operation()), operation.operation());
            }
        }
        assertEquals(identities, familyIdentities);
    }

    @Test
    void typedAndUnknownVariantsRetainDiscriminator() {
        var value = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "unknown")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("kind", "u64")
                        .put("value", "18446744073709551615"));
        assertEquals(value, JsonSupport.fromWire(JsonSupport.wire(value)));
        var path = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "path")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("encoding", "utf8")
                        .put("path", "/tmp/a"));
        assertEquals(path, JsonSupport.fromWire(path));

        var record = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "record")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("type_id", "example.v1")
                        .set("fields", JsonSupport.MAPPER.createArrayNode()));
        assertEquals(record, JsonSupport.fromWire(record));

        var variant = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "variant")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("type_id", "example.v1")
                        .put("variant", "ready")
                        .set("fields", JsonSupport.MAPPER.createArrayNode()));
        assertEquals(variant, JsonSupport.fromWire(variant));
    }

    @Test
    void scalarHelpersEmitCanonicalLosslessShapes() {
        assertEquals("18446744073709551615", WireValues.u64(JsonSupport.MAX_U64).path("value").asText());
        assertEquals("-9223372036854775808", WireValues.i64(JsonSupport.MIN_I64).path("value").asText());
        assertEquals("AP8", WireValues.bytes(new byte[] {0, (byte) 0xff})
                .path("value").path("base64").asText());
        assertEquals("/tmp/a", WireValues.utf8Path("/tmp/a").path("value").path("path").asText());
        assertEquals("2", WireValues.duration(BigInteger.TWO, 3).path("value").path("seconds").asText());
        assertEquals("-1", WireValues.timestamp(BigInteger.ONE.negate(), 4)
                .path("value").path("unix_seconds").asText());
        assertThrows(IllegalArgumentException.class, () -> WireValues.u64(JsonSupport.MAX_U64.add(BigInteger.ONE)));
        assertThrows(IllegalArgumentException.class, () -> WireValues.utf8Path("relative"));
    }

    @Test
    void decodingRejectsMalformedHandlesAndIntegerText() {
        var malformedHandle = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "handle")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("id", "")
                        .put("generation", "1")
                        .put("kind", "session"));
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(malformedHandle));
        var leadingZero = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "u64")
                .put("value", "01");
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(leadingZero));
        var overflow = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "i64")
                .put("value", "9223372036854775808");
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(overflow));

        var numericNode = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "u64")
                .put("value", 42);
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(numericNode));

        var negativeUnsigned = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "u64")
                .put("value", "-1");
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(negativeUnsigned));

        var underflow = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "i64")
                .put("value", "-9223372036854775809");
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(underflow));

        var malformedGeneration = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "handle")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("id", "run-1")
                        .put("generation", "18446744073709551616")
                        .put("kind", "run"));
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(malformedGeneration));

        var numericGeneration = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "handle")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("id", "run-1")
                        .put("generation", 1)
                        .put("kind", "run"));
        assertThrows(IllegalArgumentException.class, () -> JsonSupport.fromWire(numericGeneration));
        assertThrows(IllegalArgumentException.class, () -> new WireHandle("run-1", "01", "run"));
        assertThrows(IllegalArgumentException.class, () -> new WireHandle("run-1", "1", "unknown"));
        assertThrows(IllegalArgumentException.class, () -> new WireHandle("run-1", "18446744073709551616", "run"));
    }

    @Test
    void decodingAcceptsIntegerWireBoundariesWithoutConvertingToJsonNumbers() {
        var minimum = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "i64")
                .put("value", JsonSupport.MIN_I64.toString());
        var maximum = JsonSupport.MAPPER.createObjectNode()
                .put("kind", "u64")
                .put("value", JsonSupport.MAX_U64.toString());
        assertEquals(JsonSupport.MIN_I64.toString(), JsonSupport.fromWire(minimum).textValue());
        assertEquals(JsonSupport.MAX_U64.toString(), JsonSupport.fromWire(maximum).textValue());
    }
}
