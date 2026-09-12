package com.echoagent.sdk;

import java.time.Instant;
import java.time.OffsetDateTime;
import java.time.ZoneId;
import java.time.ZoneOffset;
import java.time.format.DateTimeParseException;

/** Platform clock and local RFC3339 helpers without owning runtime state. */
public final class TimeValues {
    private TimeValues() {}

    public static long nowSecs() { return Instant.now().getEpochSecond(); }
    public static long nowMillis() { return Instant.now().toEpochMilli(); }
    public static OffsetDateTime nowLocal() { return toLocal(Instant.now()); }
    public static OffsetDateTime toLocal(Instant value) {
        return value.atZone(ZoneId.systemDefault()).toOffsetDateTime();
    }
    public static String localRfc3339Serialize(Instant value) {
        OffsetDateTime local = toLocal(value);
        String offset = local.getOffset().getId();
        if ("Z".equals(offset)) offset = "+00:00";
        return local.toLocalDateTime() + offset;
    }
    public static Instant localRfc3339Deserialize(String value) {
        try {
            return OffsetDateTime.parse(value).toInstant();
        } catch (DateTimeParseException error) {
            throw new IllegalArgumentException("invalid RFC3339 timestamp", error);
        }
    }
    public static String optionLocalRfc3339Serialize(Instant value) {
        return value == null ? null : localRfc3339Serialize(value);
    }
    public static Instant optionLocalRfc3339Deserialize(String value) {
        return value == null ? null : localRfc3339Deserialize(value);
    }
}
