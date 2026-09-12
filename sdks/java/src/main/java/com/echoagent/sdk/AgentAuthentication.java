package com.echoagent.sdk;

import java.util.List;

/** A2A authentication configuration. */
public final class AgentAuthentication {
    private final List<AuthenticationScheme> schemes;

    public AgentAuthentication(List<AuthenticationScheme> schemes) {
        if (schemes == null || schemes.stream().anyMatch(value -> value == null)) {
            throw new IllegalArgumentException("authentication schemes must not be null");
        }
        this.schemes = List.copyOf(schemes);
    }

    public List<AuthenticationScheme> schemes() { return schemes; }
}
