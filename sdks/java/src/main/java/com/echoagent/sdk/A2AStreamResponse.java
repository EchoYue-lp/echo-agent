package com.echoagent.sdk;

/** Immutable A2A stream JSON-RPC response value. */
public final class A2AStreamResponse {
    private final String jsonrpc;
    private final String id;
    private final A2AStreamEvent result;
    private final A2AError error;

    private A2AStreamResponse(String id, A2AStreamEvent result, A2AError error) {
        if (id == null) throw new IllegalArgumentException("stream response id must not be null");
        this.jsonrpc = "2.0";
        this.id = id;
        this.result = result;
        this.error = error;
    }

    public static A2AStreamResponse newResponse(String id, A2AStreamEvent result, A2AError error) {
        return new A2AStreamResponse(id, result, error);
    }

    public String jsonrpc() { return jsonrpc; }
    public String id() { return id; }
    public A2AStreamEvent result() { return result; }
    public A2AError error() { return error; }
}
