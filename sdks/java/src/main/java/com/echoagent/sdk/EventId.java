package com.echoagent.sdk;

import java.util.Objects;

/** Validated event identity projected without owning event storage. */
public final class EventId {
    private final String value;

    private EventId(String value) {
        this.value = requireNonBlank(value, "event_id");
    }

    public static EventId newId(String value) { return new EventId(value); }

    public String asStr() { return value; }

    @Override
    public String toString() { return value; }

    private static String requireNonBlank(String value, String label) {
        Objects.requireNonNull(value, label);
        if (value.isBlank()) throw new IllegalArgumentException(label + " must not be empty");
        return value;
    }
}
