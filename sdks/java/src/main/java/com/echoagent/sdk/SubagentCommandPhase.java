package com.echoagent.sdk;

/** Durable phase of one Subagent command, without owning its receipt. */
public enum SubagentCommandPhase {
    PERSISTED("persisted"), MAILBOX_ACCEPTED("mailbox_accepted"),
    DRAINED("drained"), TURN_SETTLED("turn_settled");

    private final String wireName;

    SubagentCommandPhase(String wireName) { this.wireName = wireName; }

    public String asStr() { return wireName; }

    public static SubagentCommandPhase parse(String value) {
        if (value == null) return null;
        for (var phase : values()) {
            if (phase.wireName.equals(value)) return phase;
        }
        return null;
    }

    @Override
    public String toString() { return wireName; }
}
