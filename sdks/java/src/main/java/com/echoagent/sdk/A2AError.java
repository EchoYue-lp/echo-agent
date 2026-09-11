package com.echoagent.sdk;

/** Immutable A2A error value. */
public record A2AError(int code, String message) {
    public A2AError {
        if (message == null) throw new IllegalArgumentException("A2A error message must not be null");
    }

    public static A2AError newError(int code, String message) {
        return new A2AError(code, message);
    }
}
