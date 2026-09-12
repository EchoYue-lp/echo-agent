package com.echoagent.sdk;

/** A2A capability flags. */
public record AgentCapabilities(boolean streaming, boolean pushNotifications,
                                boolean stateTransitionHistory) {
    public AgentCapabilities() { this(false, false, false); }
}
