package com.echoagent.sdk;

/** ACP runtime values projected without owning connection or ledger state. */
public final class AcpValues {
    private AcpValues() {}

    public enum ConnectionMode {
        STANDARD("standard"), EXTENDED("extended");

        private final String wireName;

        ConnectionMode(String wireName) { this.wireName = wireName; }

        public String asStr() { return wireName; }

        @Override public String toString() { return wireName; }
    }

    public enum ExtensionSettlement {
        ANSWERED("answered"), TIMED_OUT("timed_out"), CANCELLED("cancelled"), DISCONNECTED("disconnected");

        private final String wireName;

        ExtensionSettlement(String wireName) { this.wireName = wireName; }

        public String asStr() { return wireName; }

        public boolean isAnswered() { return this == ANSWERED; }

        @Override public String toString() { return wireName; }
    }

    public enum ExtensionLeaseError {
        ADMISSION_CLOSED("extension admission is closed"),
        CONCURRENCY_LIMIT("extension concurrency limit reached"),
        EXCLUSIVE_CONFLICT("extension is already executing an exclusive invocation");

        private final String message;

        ExtensionLeaseError(String message) { this.message = message; }

        public String asStr() { return message; }

        @Override public String toString() { return message; }
    }

    public record AcpLedgerLimits(long maxEvents, long maxBytes) {
        public AcpLedgerLimits {
            if (maxEvents < 0 || maxBytes < 0) {
                throw new IllegalArgumentException("ACP ledger limits must be non-negative");
            }
        }

        public static AcpLedgerLimits defaults() {
            return new AcpLedgerLimits(10_000L, 8L * 1024L * 1024L);
        }
    }
}
