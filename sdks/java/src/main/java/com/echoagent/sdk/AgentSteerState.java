package com.echoagent.sdk;

/** Immutable steering lifecycle state; the Agent turn remains Host-owned. */
public sealed interface AgentSteerState
        permits AgentSteerState.Accepted, AgentSteerState.Drained, AgentSteerState.TurnSettled {
    AgentSteerPhase phase();

    boolean wasDrained();

    static AgentSteerState accepted() { return new Accepted(); }

    static AgentSteerState drained() { return new Drained(); }

    static AgentSteerState turnSettled(AgentSteerTurnOutcome outcome, boolean drained) {
        return new TurnSettled(outcome, drained);
    }

    record Accepted() implements AgentSteerState {
        @Override public AgentSteerPhase phase() { return AgentSteerPhase.ACCEPTED; }

        @Override public boolean wasDrained() { return false; }
    }

    record Drained() implements AgentSteerState {
        @Override public AgentSteerPhase phase() { return AgentSteerPhase.DRAINED; }

        @Override public boolean wasDrained() { return true; }
    }

    record TurnSettled(AgentSteerTurnOutcome outcome, boolean drained) implements AgentSteerState {
        public TurnSettled {
            if (outcome == null) throw new IllegalArgumentException("outcome is required");
        }

        @Override public AgentSteerPhase phase() { return AgentSteerPhase.TURN_SETTLED; }

        @Override public boolean wasDrained() { return drained; }
    }
}
