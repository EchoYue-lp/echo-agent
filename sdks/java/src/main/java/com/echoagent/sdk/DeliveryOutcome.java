package com.echoagent.sdk;

/** Terminal outcome values projected without owning the delivery ledger. */
public enum DeliveryOutcome {
    COMPLETED("completed"), FAILED("failed"), CANCELLED("cancelled"),
    DROPPED("dropped"), OUTCOME_UNKNOWN("outcome_unknown");

    private final String wireName;

    DeliveryOutcome(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    @Override
    public String toString() { return wireName; }
}
