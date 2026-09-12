package com.echoagent.sdk;

/** Coarse lifecycle phase observed for one accepted steering input. */
public enum AgentSteerPhase {
    ACCEPTED("accepted"), DRAINED("drained"), TURN_SETTLED("turn_settled");

    private final String wireName;

    AgentSteerPhase(String wireName) { this.wireName = wireName; }

    @Override
    public String toString() { return wireName; }
}
