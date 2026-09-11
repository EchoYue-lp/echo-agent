package com.echoagent.sdk;

/** Closed A2A task state with the Rust transition and display semantics. */
public enum TaskState {
    SUBMITTED("submitted"),
    WORKING("working"),
    INPUT_REQUIRED("input-required"),
    COMPLETED("completed"),
    FAILED("failed"),
    CANCELED("canceled");

    private final String wireName;

    TaskState(String wireName) { this.wireName = wireName; }

    public boolean isTerminal() {
        return this == COMPLETED || this == FAILED || this == CANCELED;
    }

    public boolean canTransitionTo(TaskState next) {
        if (next == null) throw new IllegalArgumentException("A2A task state must not be null");
        if (isTerminal()) return false;
        return (this == SUBMITTED && (next == WORKING || next == CANCELED))
                || (this == WORKING && (next == COMPLETED || next == FAILED
                || next == INPUT_REQUIRED || next == CANCELED))
                || (this == INPUT_REQUIRED && (next == WORKING || next == CANCELED));
    }

    @Override
    public String toString() { return wireName; }
}
