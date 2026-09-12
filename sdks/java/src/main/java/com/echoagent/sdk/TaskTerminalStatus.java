package com.echoagent.sdk;

/** Task terminal status values projected without owning task execution. */
public enum TaskTerminalStatus {
    COMPLETED("completed"), FAILED("failed"), CANCELLED("cancelled"),
    TIMED_OUT("timed_out"), SKIPPED("skipped");

    private final String wireName;

    TaskTerminalStatus(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override
    public String toString() { return wireName; }
}
