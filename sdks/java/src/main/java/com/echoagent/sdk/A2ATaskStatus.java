package com.echoagent.sdk;

import com.fasterxml.jackson.databind.node.ObjectNode;

import java.time.OffsetDateTime;

/** A2A task status value with a Rust-compatible timestamp projection. */
public final class A2ATaskStatus {
    private final TaskState state;
    private final A2AMessage message;
    private final String timestamp;

    private A2ATaskStatus(TaskState state, A2AMessage message) {
        if (state == null) throw new IllegalArgumentException("A2A task state must not be null");
        this.state = state;
        this.message = message;
        this.timestamp = OffsetDateTime.now().toString();
    }

    public static A2ATaskStatus newStatus(TaskState state) { return new A2ATaskStatus(state, null); }

    public static A2ATaskStatus withMessage(TaskState state, A2AMessage message) {
        if (message == null) throw new IllegalArgumentException("A2A task status message must not be null");
        return new A2ATaskStatus(state, message);
    }

    public TaskState state() { return state; }
    public A2AMessage message() { return message; }
    public String timestamp() { return timestamp; }

    public ObjectNode toJson() {
        var result = JsonSupport.MAPPER.createObjectNode().put("state", state.toString()).put("timestamp", timestamp);
        if (message == null) result.putNull("message"); else result.set("message", message.toJson());
        return result;
    }
}
