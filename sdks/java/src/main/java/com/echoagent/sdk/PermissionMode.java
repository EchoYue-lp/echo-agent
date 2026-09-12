package com.echoagent.sdk;

/** Permission mode identifiers and policy helpers projected without evaluation. */
public enum PermissionMode {
    DEFAULT("default"), PLAN("plan"), ACCEPT_EDITS("auto-edit"),
    BYPASS_PERMISSIONS("full-auto"), AUTO("auto"), BUBBLE("bubble"),
    DONT_ASK("dont-ask"), STRICT_CONFIRM("strict");

    private final String wireName;

    PermissionMode(String wireName) { this.wireName = wireName; }

    public static PermissionMode parse(String value) {
        if (value == null) throw new IllegalArgumentException("permission mode must be text");
        return switch (value.trim().toLowerCase(java.util.Locale.ROOT)) {
            case "default", "ask" -> DEFAULT;
            case "plan" -> PLAN;
            case "auto-edit", "autoedit", "accept-edits", "acceptedits" -> ACCEPT_EDITS;
            case "full-auto", "fullauto", "bypass", "bypass-permissions", "bypasspermissions" -> BYPASS_PERMISSIONS;
            case "auto" -> AUTO;
            case "bubble" -> BUBBLE;
            case "dont-ask", "dontask" -> DONT_ASK;
            case "strict", "strict-confirm", "strict-confirmation" -> STRICT_CONFIRM;
            default -> throw new IllegalArgumentException("invalid permission mode '" + value + "'");
        };
    }

    public boolean allowsWrite() { return this == BYPASS_PERMISSIONS || this == ACCEPT_EDITS; }

    public boolean requiresInteraction() {
        return this != BYPASS_PERMISSIONS && this != AUTO && this != DONT_ASK && this != ACCEPT_EDITS;
    }

    public boolean usesClassifier() { return this == AUTO; }

    @Override
    public String toString() { return wireName; }
}
