package com.echoagent.sdk;

/** Immutable A2A task parameters value. */
public final class A2ATaskParams {
    private final String id;
    private final String sessionId;
    private final A2AMessage message;

    private A2ATaskParams(A2AMessage message, String id, String sessionId) {
        if (message == null) throw new IllegalArgumentException("message must not be null");
        this.message = message;
        this.id = id;
        this.sessionId = sessionId;
    }

    public static A2ATaskParams newParams(A2AMessage message, String id, String sessionId) {
        return new A2ATaskParams(message, id, sessionId);
    }
    public String id() { return id; }
    public String sessionId() { return sessionId; }
    public A2AMessage message() { return message; }
}
