package com.echoagent.sdk;

/** Terminal outcome of the root turn that owned a steering input. */
public enum AgentSteerTurnOutcome {
    COMPLETED("completed"), CANCELLED("cancelled"), FAILED("failed"), DROPPED("dropped");

    private final String wireName;

    AgentSteerTurnOutcome(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    public static AgentSteerTurnOutcome parse(String value) {
        if (value == null) return null;
        for (var outcome : values()) {
            if (outcome.wireName.equals(value)) return outcome;
        }
        return null;
    }

    @Override
    public String toString() { return wireName; }
}
