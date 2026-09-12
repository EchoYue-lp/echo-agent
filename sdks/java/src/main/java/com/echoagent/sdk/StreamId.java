package com.echoagent.sdk;

import java.util.Objects;

/** Validated event stream identity projected without owning a live stream. */
public final class StreamId {
    private final String value;

    private StreamId(String value) {
        this.value = requireNonBlank(value, "event stream_id");
    }

    public static StreamId newId(String value) { return new StreamId(value); }

    public String asStr() { return value; }

    @Override
    public String toString() { return value; }

    private static String requireNonBlank(String value, String label) {
        Objects.requireNonNull(value, label);
        if (value.isBlank()) throw new IllegalArgumentException(label + " must not be empty");
        return value;
    }
}
