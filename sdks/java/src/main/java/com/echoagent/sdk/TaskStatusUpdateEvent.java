package com.echoagent.sdk;

/** A2A task status stream event. */
public record TaskStatusUpdateEvent(String taskId, A2ATaskStatus status, boolean isFinal) {
    public TaskStatusUpdateEvent {
        if (taskId == null || status == null) throw new IllegalArgumentException("task id and status must not be null");
    }
}
