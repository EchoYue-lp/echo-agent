package com.echoagent.sdk;

/** Command-cell renderer phases projected without owning execution. */
public enum CommandCellPhase {
    PREPARED("prepared"), QUEUED("queued"), RUNNING("running"),
    SUCCEEDED("succeeded"), FAILED("failed"), CANCELLED("cancelled"),
    LAUNCH_FAILED("launch_failed");

    private final String wireName;

    CommandCellPhase(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    public boolean isTerminal() {
        return this == SUCCEEDED || this == FAILED || this == CANCELLED || this == LAUNCH_FAILED;
    }

    @Override
    public String toString() { return wireName; }
}
