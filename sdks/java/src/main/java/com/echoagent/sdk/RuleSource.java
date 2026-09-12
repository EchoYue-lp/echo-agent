package com.echoagent.sdk;

/** Permission rule source values projected without owning evaluation order. */
public enum RuleSource {
    DEFAULT("default"), LOCAL_SETTINGS("localSettings"), PROJECT_SETTINGS("projectSettings"),
    USER_SETTINGS("userSettings"), MANAGED("managed"), CLI_ARG("cliArg"), SESSION("session");

    private final String wireName;

    RuleSource(String wireName) { this.wireName = wireName; }

    public static RuleSource parse(String value) {
        if (value == null) throw new IllegalArgumentException("rule source must be text");
        return switch (value) {
            case "default" -> DEFAULT;
            case "localSettings", "local_settings" -> LOCAL_SETTINGS;
            case "projectSettings", "project_settings" -> PROJECT_SETTINGS;
            case "userSettings", "user_settings", "manual" -> USER_SETTINGS;
            case "managed" -> MANAGED;
            case "cliArg", "cli_arg" -> CLI_ARG;
            case "session" -> SESSION;
            default -> throw new IllegalArgumentException("unknown permission rule source: " + value);
        };
    }

    @Override
    public String toString() { return wireName; }
}
