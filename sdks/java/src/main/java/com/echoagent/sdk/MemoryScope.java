package com.echoagent.sdk;

import java.util.List;
import java.util.Locale;

/** Memory lifetime scopes with Rust-compatible wire names and priority. */
public enum MemoryScope {
    USER("user", 0, true),
    PROJECT("project", 1, true),
    REPO("repo", 2, true),
    TASK("task", 3, false),
    SESSION("session", 4, false),
    RUN("run", 5, false);

    private static final List<MemoryScope> ALL = List.of(values());
    private final String wireName;
    private final int priority;
    private final boolean persistent;

    MemoryScope(String wireName, int priority, boolean persistent) {
        this.wireName = wireName;
        this.priority = priority;
        this.persistent = persistent;
    }

    public static List<MemoryScope> all() { return ALL; }
    public String wireName() { return wireName; }
    public int priority() { return priority; }
    public boolean isPersistent() { return persistent; }

    public static MemoryScope parse(String value) {
        if (value == null) return null;
        return switch (value.trim().toLowerCase(Locale.ROOT)) {
            case "user" -> USER;
            case "project", "proj" -> PROJECT;
            case "repo" -> REPO;
            case "task" -> TASK;
            case "session", "sess" -> SESSION;
            case "run" -> RUN;
            default -> null;
        };
    }

    @Override public String toString() { return wireName; }
}
