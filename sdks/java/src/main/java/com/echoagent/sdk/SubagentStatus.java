package com.echoagent.sdk;

/** Runtime-owned terminal status values projected without owning dispatch state. */
public enum SubagentStatus {
    RUNNING("running"), COMPLETED("completed"), FAILED("failed"),
    CANCELLED("cancelled"), TIMED_OUT("timed_out");

    private final String wireName;

    SubagentStatus(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    public static SubagentStatus parse(String value) {
        if (value == null) throw new IllegalArgumentException("subagent status must be text");
        for (var status : values()) {
            if (status.wireName.equals(value)) return status;
        }
        throw new IllegalArgumentException("unknown Subagent status: " + value);
    }

    @Override
    public String toString() { return wireName; }
}
