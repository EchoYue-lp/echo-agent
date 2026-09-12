package com.echoagent.sdk;

/** User-facing reasoning effort levels with Rust-compatible aliases. */
public enum ThinkingLevel {
    NONE("none"), MINIMAL("minimal"), LOW("low"), MEDIUM("medium"), HIGH("high"), XHIGH("xhigh"), MAX("max");

    private final String wireName;

    ThinkingLevel(String wireName) { this.wireName = wireName; }

    public static ThinkingLevel parse(String value) {
        if (value == null) return null;
        return switch (value.trim().toLowerCase(java.util.Locale.ROOT)) {
            case "none", "off" -> NONE;
            case "minimal", "min" -> MINIMAL;
            case "low" -> LOW;
            case "medium", "med", "normal" -> MEDIUM;
            case "high" -> HIGH;
            case "xhigh" -> XHIGH;
            case "max" -> MAX;
            default -> null;
        };
    }

    @Override
    public String toString() { return wireName; }
}
