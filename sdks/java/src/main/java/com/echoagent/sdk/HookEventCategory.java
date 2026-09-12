package com.echoagent.sdk;

/** Hook event matcher categories projected as local values. */
public enum HookEventCategory {
    TOOL("Tool"), LIFECYCLE("Lifecycle"), SUBAGENT("Subagent"), TASK("Task"),
    ERROR("Error"), EVOLUTION("Evolution");

    private final String wireName;

    HookEventCategory(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override
    public String toString() { return wireName; }
}
