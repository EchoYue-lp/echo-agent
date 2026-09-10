package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

import java.math.BigInteger;
import java.util.Base64;
import java.util.Map;

/** Public constructors for lossless scalar values on the echo-agent wire. */
public final class WireValues {
    private WireValues() {}

    /** Encodes a Java value with the SDK's lossless WireValue algebra. */
    public static ObjectNode value(Object input) {
        return JsonSupport.wire(input);
    }

    /** Encodes a typed structural record with lossless field values. */
    public static ObjectNode record(String typeId, Map<String, ?> fields) {
        return structural("record", typeId, null, fields);
    }

    /** Encodes a typed structural variant with lossless field values. */
    public static ObjectNode variant(String typeId, String variant, Map<String, ?> fields) {
        return structural("variant", typeId, variant, fields);
    }

    public static ObjectNode record(String typeId) {
        return record(typeId, Map.of());
    }

    public static ObjectNode variant(String typeId, String variant) {
        return variant(typeId, variant, Map.of());
    }

    public static ObjectNode u64(BigInteger value) {
        return integer("u64", value, BigInteger.ZERO, JsonSupport.MAX_U64);
    }

    public static ObjectNode u64(String value) {
        return u64(parseCanonical(value));
    }

    public static ObjectNode i64(BigInteger value) {
        return integer("i64", value, JsonSupport.MIN_I64, BigInteger.ONE.shiftLeft(63).subtract(BigInteger.ONE));
    }

    public static ObjectNode i64(String value) {
        return i64(parseCanonical(value));
    }

    public static ObjectNode bytes(byte[] value) {
        return JsonSupport.MAPPER.createObjectNode()
                .put("kind", "bytes")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("base64", Base64.getEncoder().withoutPadding().encodeToString(value)));
    }

    public static ObjectNode utf8Path(String path) {
        if (path == null || path.indexOf('\0') >= 0 || !(path.startsWith("/")
                || path.startsWith("\\\\")
                || path.matches("[A-Za-z]:[\\\\/].*"))) {
            throw new IllegalArgumentException("wire UTF-8 path must be absolute and NUL-free");
        }
        return JsonSupport.MAPPER.createObjectNode()
                .put("kind", "path")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("encoding", "utf8")
                        .put("path", path));
    }

    public static ObjectNode duration(BigInteger seconds, int nanos) {
        if (nanos < 0 || nanos >= 1_000_000_000) {
            throw new IllegalArgumentException("wire duration nanos must be in [0, 1_000_000_000)");
        }
        return JsonSupport.MAPPER.createObjectNode()
                .put("kind", "duration")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("seconds", u64(seconds).path("value").asText())
                        .put("nanos", nanos));
    }

    public static ObjectNode timestamp(BigInteger unixSeconds, int nanos) {
        if (nanos < 0 || nanos >= 1_000_000_000) {
            throw new IllegalArgumentException("wire timestamp nanos must be in [0, 1_000_000_000)");
        }
        return JsonSupport.MAPPER.createObjectNode()
                .put("kind", "timestamp")
                .set("value", JsonSupport.MAPPER.createObjectNode()
                        .put("unix_seconds", i64(unixSeconds).path("value").asText())
                        .put("nanos", nanos));
    }

    private static ObjectNode integer(String kind, BigInteger value, BigInteger minimum, BigInteger maximum) {
        if (value == null || value.compareTo(minimum) < 0 || value.compareTo(maximum) > 0) {
            throw new IllegalArgumentException("integer is outside the " + kind + " wire range");
        }
        return JsonSupport.MAPPER.createObjectNode().put("kind", kind).put("value", value.toString());
    }

    private static ObjectNode structural(
            String kind, String typeId, String variant, Map<String, ?> fields) {
        if (typeId == null || typeId.isBlank() || typeId.codePointCount(0, typeId.length()) > 512) {
            throw new IllegalArgumentException("wire structural type_id must be non-empty and <= 512 characters");
        }
        if ("variant".equals(kind)
                && (variant == null || variant.isBlank() || variant.codePointCount(0, variant.length()) > 256)) {
            throw new IllegalArgumentException("wire variant discriminator must be non-empty and <= 256 characters");
        }
        if (fields == null) throw new IllegalArgumentException("wire structural fields must not be null");
        var payload = JsonSupport.MAPPER.createObjectNode().put("type_id", typeId);
        if (variant != null) payload.put("variant", variant);
        var fieldArray = payload.putArray("fields");
        for (var entry : fields.entrySet()) {
            String name = entry.getKey();
            if (name == null || name.isBlank() || name.codePointCount(0, name.length()) > 256) {
                throw new IllegalArgumentException("wire structural field names must be non-empty and <= 256 characters");
            }
            fieldArray.add(JsonSupport.MAPPER.createObjectNode()
                    .put("name", name)
                    .set("value", JsonSupport.wire(entry.getValue())));
        }
        return JsonSupport.MAPPER.createObjectNode().put("kind", kind).set("value", payload);
    }

    private static BigInteger parseCanonical(String value) {
        if (value == null || value.isBlank() || value.equals("-0")
                || (value.length() > 1 && value.charAt(0) == '0')
                || (value.length() > 2 && value.startsWith("-0"))) {
            throw new IllegalArgumentException("wire integer string must be canonical decimal text");
        }
        try {
            if (!value.matches("-?[0-9]+")) throw new NumberFormatException();
            return new BigInteger(value);
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException("wire integer string must be canonical decimal text", error);
        }
    }
}
