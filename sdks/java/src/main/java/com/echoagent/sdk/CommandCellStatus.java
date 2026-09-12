package com.echoagent.sdk;

/** Command-cell terminal/artifact status values projected without ownership. */
public final class CommandCellStatus {
    private CommandCellStatus() {}

    public enum TerminalCause {
        EXITED("exited"), TIMED_OUT("timed_out"), CANCELLED("cancelled"),
        LAUNCH_FAILED("launch_failed"), WAIT_FAILED("wait_failed"),
        OUTPUT_DRAIN_FAILED("output_drain_failed");

        private final String wireName;

        TerminalCause(String wireName) { this.wireName = wireName; }

        public String asStr() { return wireName; }

        @Override public String toString() { return wireName; }
    }

    public enum ArtifactStatus {
        NOT_REQUESTED("not_requested"), WRITING("writing"),
        BELOW_THRESHOLD("below_threshold"), AVAILABLE("available"), FAILED("failed");

        private final String wireName;

        ArtifactStatus(String wireName) { this.wireName = wireName; }

        public String asStr() { return wireName; }

        @Override public String toString() { return wireName; }
    }
}
