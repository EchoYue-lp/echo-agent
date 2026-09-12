package com.echoagent.sdk;

/** Skill source values projected without loading or executing skills. */
public enum SkillSource {
    LOCAL("local"), MCP("mcp");

    private final String wireName;

    SkillSource(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override public String toString() { return wireName; }
}
