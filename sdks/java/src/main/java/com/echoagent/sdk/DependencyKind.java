package com.echoagent.sdk;

/** Skill dependency kinds projected without probing the host. */
public enum DependencyKind {
    BINARY("binary"), PYTHON_PKG("python_pkg"), NODE_MODULE("node_module");

    private final String wireName;

    DependencyKind(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override public String toString() { return wireName; }
}
