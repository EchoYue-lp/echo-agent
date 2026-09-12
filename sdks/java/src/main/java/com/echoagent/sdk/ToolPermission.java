package com.echoagent.sdk;

/** Permission categories used by RuleMatcher values. */
public enum ToolPermission {
    READ("read"), WRITE("write"), NETWORK("network"), EXECUTE("execute"), SENSITIVE("sensitive");

    private final String wireName;

    ToolPermission(String wireName) { this.wireName = wireName; }

    @Override
    public String toString() { return wireName; }
}
