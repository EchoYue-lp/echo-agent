package com.echoagent.sdk;

/** Terminal Subagent hook status values projected without owning hooks. */
public enum SubagentStopStatus {
    COMPLETED("completed"), FAILED("failed"), CANCELLED("cancelled"), TIMED_OUT("timed_out");

    private final String wireName;

    SubagentStopStatus(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override
    public String toString() { return wireName; }
}
