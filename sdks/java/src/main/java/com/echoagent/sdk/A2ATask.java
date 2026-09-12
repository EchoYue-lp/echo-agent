package com.echoagent.sdk;

import java.util.List;

/** Immutable A2A task value. */
public final class A2ATask {
    private final String id;
    private final String sessionId;
    private final A2ATaskStatus status;
    private final List<A2AMessage> history;
    private final List<A2AArtifact> artifacts;

    private A2ATask(String id, A2ATaskStatus status, String sessionId,
                    List<A2AMessage> history, List<A2AArtifact> artifacts) {
        if (id == null || status == null) throw new IllegalArgumentException("task id and status must not be null");
        this.id = id;
        this.status = status;
        this.sessionId = sessionId;
        this.history = List.copyOf(history == null ? List.of() : history);
        this.artifacts = List.copyOf(artifacts == null ? List.of() : artifacts);
    }

    public static A2ATask newTask(String id, A2ATaskStatus status, String sessionId,
                                  List<A2AMessage> history, List<A2AArtifact> artifacts) {
        return new A2ATask(id, status, sessionId, history, artifacts);
    }
    public String id() { return id; }
    public String sessionId() { return sessionId; }
    public A2ATaskStatus status() { return status; }
    public List<A2AMessage> history() { return history; }
    public List<A2AArtifact> artifacts() { return artifacts; }
}
