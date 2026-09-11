package com.echoagent.sdk;

/** A2A artifact stream event. */
public record TaskArtifactUpdateEvent(String taskId, A2AArtifact artifact, boolean isFinal) {
    public TaskArtifactUpdateEvent {
        if (taskId == null || artifact == null) throw new IllegalArgumentException("task id and artifact must not be null");
    }
}
