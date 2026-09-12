package com.echoagent.sdk;

/** Observed Subagent isolation value without owning isolation execution. */
public final class ObservedIsolation {
    private final String value;

    private ObservedIsolation(String value) { this.value = value; }

    public static ObservedIsolation newValue(String value) {
        if (value == null) throw new IllegalArgumentException("observed isolation must be text");
        String trimmed = value.trim();
        if (trimmed.isEmpty()) return new ObservedIsolation("unknown");
        int count = Math.min(512, trimmed.codePointCount(0, trimmed.length()));
        return new ObservedIsolation(trimmed.substring(0, trimmed.offsetByCodePoints(0, count)));
    }

    public static ObservedIsolation defaults() { return new ObservedIsolation("unknown"); }

    public String asStr() { return value; }
}
